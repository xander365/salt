//! The four routes issue #53 adds (parent #49 Spec 2 of 3): `POST
//! .../payroll-runs`, `GET .../payroll-runs`, `GET .../payroll-runs/{r}` and
//! `PUT .../payroll-runs/{r}/members/{em}/pay-lines` (generalized from
//! `.../earnings` by issue #77, which the old route does not stand beside),
//! plus `POST .../payroll-runs/{r}/calculate` (issue #55) and `POST
//! .../payroll-runs/{r}/finalize` (issue #56). An Operator creates the
//! next Ordinary run for a period, sees the Employer's runs, opens one and
//! reads every member it proposes to pay, sets one member's earning lines,
//! calculates the run to see the figures — or the reason — for every
//! member, and finalizes it into immutable history.
//!
//! Pay lines is `PUT`, not `POST`: `payroll_app::set_run_pay_lines` replaces
//! the member's whole list, and naming that idempotence in the method is a
//! decision, not a preference (issue #53's own Deep Instructions).
//!
//! The request body contains taxable allowance and overtime instructions.
//! `BasicPay` is derived by the calculator from `CompensationTerms`, so it
//! cannot be expressed by this input boundary.
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
use axum::http::StatusCode;
use chrono::NaiveDate;
use payroll::{
    EarningInstruction, EarningLabel, EmployerId, EmploymentId, Money, OvertimeHours,
    OvertimeMultiplier, OvertimeMultiplierError,
};
use payroll_app::{
    PayrollAppError, PayrollFigures, PayrollRunBlocker, PayrollRunDetail, RunStatus,
};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::str::FromStr;

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

/// One earning instruction on the wire, tagged by `kind`:
///
/// - `{"kind": "taxableAllowance", "amountCents": 2000, "label": "standby allowance"}`
/// - `{"kind": "overtime", "hours": "12", "multiplier": "1.5", "label": "Sunday overtime"}`
///
/// A tagged enum and not one flat shape with optional fields, because the
/// two kinds genuinely carry different facts: an allowance is money an
/// Operator decided, overtime is hours Salt prices itself (D14). A body
/// naming a `kind` this enum does not have — `basicPay`, say — fails to
/// deserialize and is a malformed request, which is exactly right: `BasicPay`
/// is derived from `CompensationTerms` and can never be typed.
///
/// `hours` and `multiplier` are decimal strings, like `ordinaryHours` on the
/// compensation-terms route, so no figure passes through a JSON float
/// (INV-001).
///
/// Responses use a nullable label so a version-1 unlabelled allowance can be
/// displayed honestly and because overtime labels are optional. New taxable
/// allowances still require a valid non-blank label.
#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub(crate) enum EarningLineDto {
    #[serde(rename_all = "camelCase")]
    TaxableAllowance {
        amount_cents: i64,
        label: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Overtime {
        hours: String,
        multiplier: String,
        label: Option<String>,
    },
}

/// An overtime multiplier outside the closed set of 1.5 and 2.0 (D31).
/// Its own code rather than a bare `malformed_request`, because the body was
/// well formed and the refusal has a reason worth stating: the set is closed
/// and a third factor is a code change, not a number a caller may supply.
/// `details` names what was sent and what is supported, so the refusal is
/// actionable without the caller guessing.
///
/// Whether 1.5 and 2.0 are the correct and only statutory factors in Namibia
/// is `Q-OPEN-8` and unverified — nothing here says they are law.
fn unsupported_multiplier(refusal: OvertimeMultiplierError, supplied: &str) -> ApiError {
    // Destructured rather than ignored, so a second `OvertimeMultiplierError`
    // variant fails this build instead of silently reporting itself under
    // this one code.
    let OvertimeMultiplierError::Unsupported { .. } = refusal;
    ApiError::payroll_refusal(
        StatusCode::UNPROCESSABLE_ENTITY,
        "unsupported_overtime_multiplier",
        refusal.to_string(),
        Some(serde_json::json!({
            "supplied": supplied,
            "supported": ["1.5", "2"],
        })),
    )
}

/// Overtime hours that are zero, negative, finer than a hundredth, or larger
/// than a whole pay period. Its own code for the same reason as
/// [`unsupported_multiplier`]: the body parsed, and the reason is worth
/// stating rather than collapsing into "malformed".
fn invalid_overtime_hours(message: String, supplied: &str) -> ApiError {
    ApiError::payroll_refusal(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_overtime_hours",
        message,
        Some(serde_json::json!({ "supplied": supplied })),
    )
}

/// A non-blank label is required on every new line. `None` is readable on the
/// way out — a version-1 allowance really had none — but never writable.
fn parse_label(label: Option<String>) -> Result<EarningLabel, ApiError> {
    label
        .ok_or_else(ApiError::malformed_request)
        .and_then(|label| EarningLabel::new(label).map_err(|_| ApiError::malformed_request()))
}

fn parse_optional_label(label: Option<String>) -> Result<Option<EarningLabel>, ApiError> {
    match label {
        None => Ok(None),
        Some(label) if label.trim().is_empty() => Ok(None),
        Some(label) => EarningLabel::new(label)
            .map(Some)
            .map_err(|_| ApiError::malformed_request()),
    }
}

fn parse_earning(line: EarningLineDto) -> Result<EarningInstruction, ApiError> {
    match line {
        EarningLineDto::TaxableAllowance {
            amount_cents,
            label,
        } => {
            let amount =
                Money::from_cents(amount_cents).map_err(|_| ApiError::malformed_request())?;
            Ok(EarningInstruction::TaxableAllowance {
                amount,
                label: Some(parse_label(label)?),
            })
        }
        EarningLineDto::Overtime {
            hours,
            multiplier,
            label,
        } => {
            let hours_decimal =
                Decimal::from_str(&hours).map_err(|_| ApiError::malformed_request())?;
            let parsed_hours = OvertimeHours::new(hours_decimal)
                .map_err(|refusal| invalid_overtime_hours(refusal.to_string(), &hours))?;
            let multiplier_decimal =
                Decimal::from_str(&multiplier).map_err(|_| ApiError::malformed_request())?;
            let parsed_multiplier = OvertimeMultiplier::try_from(multiplier_decimal)
                .map_err(|refusal| unsupported_multiplier(refusal, &multiplier))?;
            Ok(EarningInstruction::Overtime {
                hours: parsed_hours,
                multiplier: parsed_multiplier,
                label: parse_optional_label(label)?,
            })
        }
    }
}

fn earning_to_dto(earning: EarningInstruction) -> EarningLineDto {
    match earning {
        EarningInstruction::TaxableAllowance { amount, label } => {
            EarningLineDto::TaxableAllowance {
                amount_cents: amount.cents(),
                label: label.map(|label| label.to_string()),
            }
        }
        EarningInstruction::Overtime {
            hours,
            multiplier,
            label,
        } => EarningLineDto::Overtime {
            hours: hours.as_decimal().to_string(),
            multiplier: multiplier.as_decimal().to_string(),
            label: label.map(|label| label.to_string()),
        },
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
    finalized_payroll_id: Option<String>,
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

/// The ten figures §0.29 names for a member's current calculation — shared
/// by a working run's own detail/calculate response and by
/// [`crate::finalized_payroll`]'s finalized-payroll detail (issue #57), so
/// the same ten names appear on the wire whether the run is `Calculated` or
/// already `Finalized`. Cents-exact integers on the wire, never a JSON float
/// (INV-001).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FiguresDto {
    basic_pay_cents: i64,
    taxable_allowances_cents: i64,
    overtime_cents: i64,
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
        overtime_cents: figures.overtime.cents(),
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
                    finalized_payroll_id: member.finalized_payroll_id.map(|id| id.to_string()),
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
pub(crate) struct SetRunPayLinesRequest {
    earnings: Vec<EarningLineDto>,
}

/// `PUT /api/employers/{e}/payroll-runs/{r}/members/{em}/pay-lines`: replaces
/// that member's whole list of pay lines. A run id belonging to another
/// Employer is 404, checked here before `payroll_app::set_run_pay_lines`
/// runs, since that use case takes no `EmployerId` of its own.
///
/// The request body still names its array `earnings`: every line this route
/// accepts today is an Earning, `DeductionInstruction` having no producer
/// yet (issue #78). The route itself is renamed off `.../earnings` because
/// what it replaces — and what `payroll_app::set_run_pay_lines` stores — is
/// provenance-carrying pay lines, not an earnings-only concept (issue #77).
pub(crate) async fn set_run_pay_lines(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, payroll_run_id, employment_id)): Path<(String, String, String)>,
    body: Result<Json<SetRunPayLinesRequest>, JsonRejection>,
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

    payroll_app::set_run_pay_lines(state.db(), &payroll_run_id, &employment_id, earnings).await?;

    Ok(Json(RecordedResponse {}))
}
