//! The four routes issue #53 adds (parent #49 Spec 2 of 3): `POST
//! .../payroll-runs`, `GET .../payroll-runs`, `GET .../payroll-runs/{r}` and
//! `PUT .../payroll-runs/{r}/members/{em}/earnings`. An Operator creates the
//! next Ordinary run for a period, sees the Employer's runs, opens one and
//! reads every member it proposes to pay, and sets one member's earning
//! lines — the whole working state of a run, before any arithmetic happens.
//!
//! Earnings is `PUT`, not `POST`: `payroll_app::set_run_earnings` replaces
//! the member's whole earnings list, and naming that idempotence in the
//! method is a decision, not a preference (issue #53's own Deep
//! Instructions).
//!
//! A `BasicPay` line in the request body is parsed here, not dropped or
//! pre-checked — it reaches `payroll_app::set_run_earnings`, which refuses
//! it, because that use case is what knows a `BasicPay` line is derived from
//! `CompensationTerms` and is also the social security base.
//!
//! Every handler here takes its `EmployerId` from
//! [`AuthorizedEmployerContext`] and never from the path (ADR-0017). `GET`
//! and `PUT`/`POST` `.../payroll-runs/{r}...` routes that do not already
//! take an `EmployerId` inside `payroll_app` (the earnings route) call
//! [`payroll_app::verify_payroll_run_belongs_to_employer`] first, so a run
//! id belonging to another Employer is 404 before any use case runs.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, Path, State};
use chrono::NaiveDate;
use payroll::{Earning, EmployerId, EmploymentId, Money};
use payroll_app::RunStatus;
use serde::{Deserialize, Serialize};

use crate::authorized_employer::AuthorizedEmployerContext;
use crate::employment_facts::{PayPeriodDto, RecordedResponse};
use crate::error::ApiError;
use crate::state::AppState;

/// `payroll_run.status`'s wire spelling — the same three states
/// `RunStatus::from_column` reads back, turned into the string a client
/// branches on rather than this crate serializing the domain enum directly.
fn status_str(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Draft => "draft",
        RunStatus::Calculated => "calculated",
        RunStatus::Finalized => "finalized",
    }
}

/// One classified Earning line on the wire: `{"kind": "taxableAllowance",
/// "amountCents": 2000}`. Both directions share this shape — the response
/// echoes back exactly what a request would set — so `basicPay` round-trips
/// for display even though [`parse_earning`] lets a request name it only so
/// `set_run_earnings` can refuse it, never so the handler drops it silently.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EarningLineDto {
    kind: String,
    amount_cents: i64,
}

fn parse_earning(line: EarningLineDto) -> Result<Earning, ApiError> {
    let amount = Money::from_cents(line.amount_cents).map_err(|_| ApiError::malformed_request())?;
    match line.kind.as_str() {
        "basicPay" => Ok(Earning::BasicPay(amount)),
        "taxableAllowance" => Ok(Earning::TaxableAllowance(amount)),
        _ => Err(ApiError::malformed_request()),
    }
}

fn earning_to_dto(earning: Earning) -> EarningLineDto {
    let (kind, amount) = match earning {
        Earning::BasicPay(amount) => ("basicPay", amount),
        Earning::TaxableAllowance(amount) => ("taxableAllowance", amount),
    };
    EarningLineDto {
        kind: kind.to_string(),
        amount_cents: amount.cents(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreatePayrollRunRequest {
    period: PayPeriodDto,
    pay_date: NaiveDate,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreatePayrollRunResponse {
    payroll_run_id: String,
}

/// `POST /api/employers/{e}/payroll-runs`: creates a Draft Ordinary run for
/// `period` and `payDate`. Scope boundary (issue #53's own Deep
/// Instructions): no Correction-run creation route here.
pub(crate) async fn create_payroll_run(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    body: Result<Json<CreatePayrollRunRequest>, JsonRejection>,
) -> Result<Json<CreatePayrollRunResponse>, ApiError> {
    let Json(request) = body.map_err(|_rejection| ApiError::malformed_request())?;
    let employer_id = EmployerId::new(context.employer_id().as_str());
    let period = payroll::PayPeriod::try_from(request.period)
        .map_err(|_| ApiError::malformed_request())?;

    let payroll_run_id = payroll_app::create_ordinary_payroll_run(
        state.db(),
        &employer_id,
        period,
        request.pay_date,
        &context.actor(),
    )
    .await?;

    Ok(Json(CreatePayrollRunResponse {
        payroll_run_id: payroll_run_id.to_string(),
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PayrollRunsResponse {
    payroll_runs: Vec<PayrollRunSummaryDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PayrollRunSummaryDto {
    payroll_run_id: String,
    period: PayPeriodDto,
    pay_date: NaiveDate,
    status: &'static str,
}

/// `GET /api/employers/{e}/payroll-runs`: every PayrollRun this Employer
/// has. No pagination, no filters, no sorting (issue #53's own Deep
/// Instructions).
pub(crate) async fn list_payroll_runs(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
) -> Result<Json<PayrollRunsResponse>, ApiError> {
    let employer_id = EmployerId::new(context.employer_id().as_str());

    let payroll_runs = payroll_app::list_payroll_runs(state.db(), &employer_id)
        .await?
        .into_iter()
        .map(|run| PayrollRunSummaryDto {
            payroll_run_id: run.id.to_string(),
            period: PayPeriodDto::from(run.period),
            pay_date: run.pay_date,
            status: status_str(run.status),
        })
        .collect();

    Ok(Json(PayrollRunsResponse { payroll_runs }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PayrollRunDetailResponse {
    payroll_run_id: String,
    period: PayPeriodDto,
    pay_date: NaiveDate,
    status: &'static str,
    members: Vec<PayrollRunMemberDto>,
}

/// One member of a run's detail response. No `blockers` field yet (that is
/// issue #54) — a struct of its own is what lets that field be added here
/// later rather than reshaping this response (issue #53's own Deep
/// Instructions).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PayrollRunMemberDto {
    employment_id: String,
    full_name: String,
    earnings: Vec<EarningLineDto>,
}

/// `GET /api/employers/{e}/payroll-runs/{r}`: the run's period, pay date,
/// status, and every member it proposes to pay, by name, with their current
/// Earning lines. A run id belonging to another Employer is 404
/// (ADR-0017), the same as an unknown one.
pub(crate) async fn get_payroll_run(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, payroll_run_id)): Path<(String, String)>,
) -> Result<Json<PayrollRunDetailResponse>, ApiError> {
    let employer_id = EmployerId::new(context.employer_id().as_str());

    let detail =
        payroll_app::get_payroll_run_detail(state.db(), &employer_id, &payroll_run_id).await?;

    Ok(Json(PayrollRunDetailResponse {
        payroll_run_id: detail.id.to_string(),
        period: PayPeriodDto::from(detail.period),
        pay_date: detail.pay_date,
        status: status_str(detail.status),
        members: detail
            .members
            .into_iter()
            .map(|member| PayrollRunMemberDto {
                employment_id: member.employment_id.to_string(),
                full_name: member.full_name,
                earnings: member.earnings.into_iter().map(earning_to_dto).collect(),
            })
            .collect(),
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetRunEarningsRequest {
    earnings: Vec<EarningLineDto>,
}

/// `PUT /api/employers/{e}/payroll-runs/{r}/members/{em}/earnings`: replaces
/// that member's whole Earnings list. A run id belonging to another Employer
/// is 404, checked here before `payroll_app::set_run_earnings` runs, since
/// that use case takes no `EmployerId` of its own.
pub(crate) async fn set_run_earnings(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, payroll_run_id, employment_id)): Path<(String, String, String)>,
    body: Result<Json<SetRunEarningsRequest>, JsonRejection>,
) -> Result<Json<RecordedResponse>, ApiError> {
    let Json(request) = body.map_err(|_rejection| ApiError::malformed_request())?;
    let employer_id = EmployerId::new(context.employer_id().as_str());
    let payroll_run_id = payroll_app::verify_payroll_run_belongs_to_employer(
        state.db(),
        &employer_id,
        &payroll_run_id,
    )
    .await?;
    let employment_id = EmploymentId::new(employment_id);

    let earnings = request
        .earnings
        .into_iter()
        .map(parse_earning)
        .collect::<Result<Vec<_>, _>>()?;

    payroll_app::set_run_earnings(state.db(), &payroll_run_id, &employment_id, earnings).await?;

    Ok(Json(RecordedResponse {}))
}
