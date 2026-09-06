//! The four routes issue #53 adds (parent #49 Spec 2 of 3): `POST
//! .../payroll-runs`, `GET .../payroll-runs`, `GET .../payroll-runs/{r}` and
//! `PUT .../payroll-runs/{r}/members/{em}/earnings`, plus `POST
//! .../payroll-runs/{r}/calculate` (issue #55) and `POST
//! .../payroll-runs/{r}/finalize` (issue #56). An Operator creates the
//! next Ordinary run for a period, sees the Employer's runs, opens one and
//! reads every member it proposes to pay, sets one member's earning lines,
//! calculates the run to see the figures — or the reason — for every
//! member, and finalizes it into immutable history.
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
//! take an `EmployerId` inside `payroll_app` (the earnings and calculate
//! routes) call [`payroll_app::verify_payroll_run_belongs_to_employer`]
//! first, so a run id belonging to another Employer is 404 before any use
//! case runs.
//!
//! Calculate is not an error (§0.25): `calculate_payroll_run` below always
//! answers 200 with the run detail, even when every member refused. No
//! handler here recalculates anything, inspects a figure, or decides
//! whether Finalize is allowed (issue #55's own Deep Instructions) — a
//! client reads that straight off `status`, which `payroll_app` sets to
//! `"calculated"` exactly when every active member has a current, successful
//! calculation, the one state `payroll_app::finalize_payroll_run` accepts.
//! It is the run's readiness, not a promise that Finalize cannot then refuse
//! for a reason of its own (a stale acknowledgement, a divergence): those
//! are finalization's own refusals and belong to a later ticket.

use std::collections::HashMap;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, Path, State};
use chrono::NaiveDate;
use payroll::{Earning, EmployerId, EmploymentId, Money};
use payroll_app::{
    PayrollAppError, PayrollFigures, PayrollRunBlocker, PayrollRunDetail, RunStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::authorized_employer::AuthorizedEmployerContext;
use crate::employment_facts::{PayPeriodDto, RecordedResponse};
use crate::error::ApiError;
use crate::payroll_error::{blocker_code_and_details, refusal_code_and_details};
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
    let period =
        payroll::PayPeriod::try_from(request.period).map_err(|_| ApiError::malformed_request())?;

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

/// One member of a run's detail response: who is being proposed to pay, by
/// name, their current Earning lines, and why they cannot be paid right now,
/// if at all. An empty `blockers` means ready (issue #54, §0.31).
///
/// `figures` and `refusal` (issue #55) are different things and both may be
/// present: a `blocker` is read from standing facts, a `refusal` is what
/// Calculate's own most recent call actually said. `figures` comes back on
/// this same DTO whether the response is Calculate's own or a later plain
/// `GET` refresh — both read the member's stored `WorkingPayrollCalculation`
/// — but `refusal` is never persisted, so a `GET` refresh always carries
/// `null` there even for a member still blocked.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PayrollRunMemberDto {
    employment_id: String,
    full_name: String,
    earnings: Vec<EarningLineDto>,
    blockers: Vec<BlockerDto>,
    figures: Option<FiguresDto>,
    refusal: Option<RefusalDto>,
}

/// One entry of a member's `blockers` list, under the same stable `code`s
/// [`crate::payroll_error`] already maps every refusal to (issue #54's own
/// Deep Instructions: "the same strings the mapping in #50 already owns").
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BlockerDto {
    code: &'static str,
    details: Option<Value>,
}

fn blocker_to_dto(blocker: PayrollRunBlocker) -> BlockerDto {
    let (code, details) = blocker_code_and_details(&blocker);
    BlockerDto { code, details }
}

/// The nine figures §0.29 names for a member's current calculation — shared
/// by a working run's own detail/calculate response and by
/// [`crate::finalized_payroll`]'s finalized-payroll detail (issue #57), so
/// the same nine names appear on the wire whether the run is `Calculated` or
/// already `Finalized`. Cents-exact integers on the wire, never a JSON float
/// (INV-001).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FiguresDto {
    basic_pay_cents: i64,
    taxable_allowances_cents: i64,
    gross_cents: i64,
    taxable_remuneration_cents: i64,
    paye_cents: i64,
    employee_ssc_cents: i64,
    employer_ssc_cents: i64,
    total_deductions_cents: i64,
    net_cents: i64,
}

pub(crate) fn figures_to_dto(figures: PayrollFigures) -> FiguresDto {
    FiguresDto {
        basic_pay_cents: figures.basic_pay.cents(),
        taxable_allowances_cents: figures.taxable_allowances.cents(),
        gross_cents: figures.gross.cents(),
        taxable_remuneration_cents: figures.taxable_remuneration.cents(),
        paye_cents: figures.paye.cents(),
        employee_ssc_cents: figures.employee_social_security.cents(),
        employer_ssc_cents: figures.employer_social_security.cents(),
        total_deductions_cents: figures.total_deductions.cents(),
        net_cents: figures.net_pay.cents(),
    }
}

/// A member's `refusal`, under the same stable `code`s the error envelope
/// itself uses for the same refusal (issue #55's own Deep Instructions).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RefusalDto {
    code: &'static str,
    details: Option<Value>,
}

fn refusal_to_dto(refusal: &PayrollAppError) -> RefusalDto {
    let (code, details) = refusal_code_and_details(refusal);
    RefusalDto { code, details }
}

/// Turns a [`PayrollRunDetail`] into the wire response both `GET
/// .../payroll-runs/{r}` and `POST .../payroll-runs/{r}/calculate` answer
/// with (issue #55's own Deep Instructions: one DTO, not two shapes for the
/// same run).
///
/// `refusals` is what the calculator actually said on the call that just
/// ran, named by `EmploymentId` — never a field of the read model, which
/// persists no refusal and could only ever answer `None` for one. A plain
/// `GET` therefore passes an empty map and every member's `refusal` is
/// `null`, exactly as a refresh should read.
fn payroll_run_detail_to_response(
    detail: PayrollRunDetail,
    mut refusals: HashMap<EmploymentId, PayrollAppError>,
) -> PayrollRunDetailResponse {
    let response = PayrollRunDetailResponse {
        payroll_run_id: detail.id.to_string(),
        period: PayPeriodDto::from(detail.period),
        pay_date: detail.pay_date,
        status: status_str(detail.status),
        members: detail
            .members
            .into_iter()
            .map(|member| {
                let refusal = refusals
                    .remove(&member.employment_id)
                    .map(|refusal| refusal_to_dto(&refusal));
                PayrollRunMemberDto {
                    employment_id: member.employment_id.to_string(),
                    full_name: member.full_name,
                    earnings: member.earnings.into_iter().map(earning_to_dto).collect(),
                    blockers: member.blockers.into_iter().map(blocker_to_dto).collect(),
                    figures: member.figures.map(figures_to_dto),
                    refusal,
                }
            })
            .collect(),
    };

    // A refusal left over named a member the re-read no longer lists — it
    // was removed from the run between the calculation's own commit and
    // that read. Reporting it against a member the response does not carry
    // is impossible, so it is dropped, but never silently: an Operator who
    // asks why a reason vanished needs this line in the log.
    for (employment_id, refusal) in refusals {
        tracing::warn!(
            employment_id = %employment_id,
            error = %refusal,
            "a calculation refusal named a member the run detail no longer lists"
        );
    }

    response
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

    Ok(Json(payroll_run_detail_to_response(detail, HashMap::new())))
}

/// `POST /api/employers/{e}/payroll-runs/{r}/calculate` (issue #55):
/// recalculates every active member and answers **200** with the same run
/// detail `GET` reads, whether every member calculated cleanly, some
/// refused, or all of them did — Calculate is not an error (§0.25), so a
/// refusal lives in a member's own `refusal` field, never in the response's
/// status.
///
/// Confirms `payroll_run_id` belongs to `employer_id` first (ADR-0017):
/// `payroll_app::calculate_payroll_run` takes no `EmployerId` of its own, so
/// a run id belonging to another Employer must be refused here, the same as
/// a missing one, before any calculation runs. An already-`Finalized` run
/// is refused with its own stable code from that same call.
///
/// The only thing `payroll_app::get_payroll_run_detail` itself can never
/// show is filled in after reading it back: each refused member's own
/// `refusal`, matched onto the freshly-read detail by `EmploymentId`. Every
/// other member's `figures` already comes from that read, because the
/// calculation above just wrote its `WorkingPayrollCalculation` row before
/// this handler re-reads it — the same row a later plain `GET` would also
/// see.
pub(crate) async fn calculate_payroll_run(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, payroll_run_id)): Path<(String, String)>,
) -> Result<Json<PayrollRunDetailResponse>, ApiError> {
    let employer_id = EmployerId::new(context.employer_id().as_str());
    let run_id = payroll_app::verify_payroll_run_belongs_to_employer(
        state.db(),
        &employer_id,
        &payroll_run_id,
    )
    .await?;

    let refusals_by_member: HashMap<EmploymentId, PayrollAppError> =
        payroll_app::calculate_payroll_run(state.db(), &run_id, &context.actor())
            .await?
            .into_iter()
            .map(|refusal| (refusal.employment_id, refusal.refusal))
            .collect();

    let detail =
        payroll_app::get_payroll_run_detail(state.db(), &employer_id, &payroll_run_id).await?;

    Ok(Json(payroll_run_detail_to_response(
        detail,
        refusals_by_member,
    )))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FinalizePayrollRunResponse {
    finalized: Vec<FinalizedMemberDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FinalizedMemberDto {
    employment_id: String,
    finalized_payroll_id: String,
}

/// `POST /api/employers/{e}/payroll-runs/{r}/finalize` (issue #56): the one
/// atomic act that turns a `Calculated` run into immutable history. Answers
/// **200** with each member's new `finalizedPayrollId` alongside its
/// `employmentId` — the ids an Operator needs to open what was just written.
///
/// Confirms `payroll_run_id` belongs to `employer_id` first (ADR-0017):
/// `payroll_app::finalize_payroll_run` takes no `EmployerId` of its own, so a
/// run id belonging to another Employer must be refused here, the same as a
/// missing one, before finalization runs.
///
/// This handler never re-checks a run's status, re-compares a figure, or
/// decides a run "looks finalizable" (issue #56's own Deep Instructions) — it
/// calls `payroll_app::finalize_payroll_run` and maps the outcome. Facts
/// that moved since Calculate, a retry against an already-finalized run, and
/// a run that was never calculated are all that use case's own refusals,
/// mapped to their stable codes by [`crate::payroll_error`] — a retried
/// finalize reads back `payroll_run_already_finalized` with
/// `details.finalizedPayrollId`, which is the recovery itself (§0.28): no
/// idempotency key is needed because the database's own uniqueness and the
/// run lock already make a duplicate impossible.
///
/// `FinalizationOutcome::later_finalized_periods` is deliberately not on the
/// wire: it is a Correction run's own §6.4 warning and is always empty for
/// an Ordinary run, and §0.22 gives Corrections no route at all. Putting an
/// always-empty field in the response would make a permanent contract out of
/// something no caller in this spec can ever see filled in.
pub(crate) async fn finalize_payroll_run(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, payroll_run_id)): Path<(String, String)>,
) -> Result<Json<FinalizePayrollRunResponse>, ApiError> {
    let employer_id = EmployerId::new(context.employer_id().as_str());
    let run_id = payroll_app::verify_payroll_run_belongs_to_employer(
        state.db(),
        &employer_id,
        &payroll_run_id,
    )
    .await?;

    let outcome = payroll_app::finalize_payroll_run(state.db(), &run_id, &context.actor()).await?;

    Ok(Json(FinalizePayrollRunResponse {
        finalized: outcome
            .finalized
            .into_iter()
            .map(|(employment_id, finalized_payroll_id)| FinalizedMemberDto {
                employment_id: employment_id.to_string(),
                finalized_payroll_id: finalized_payroll_id.to_string(),
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
