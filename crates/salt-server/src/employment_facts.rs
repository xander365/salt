//! The four routes issue #52 adds (parent #49 Spec 2 of 3): `POST
//! .../compensation-terms`, `.../prior-employment`, `.../unsupported-deductions`
//! and `.../opening-balance`. Together they let an Operator record everything
//! `calculate` demands of an Employment before it can be paid, with no route
//! yet for correcting any of these facts (that is `correct_compensation_terms`
//! and its siblings, out of this ticket's scope) and no route for voiding an
//! Employment.
//!
//! Each handler here authenticates and authorizes through
//! [`AuthorizedEmployerContext`], parses and validates transport shape only,
//! calls the matching `payroll_app` use case, and maps the result through
//! [`crate::payroll_error`] — it never validates a payroll rule itself, and it
//! never reads the `EmployerId` from the path (ADR-0017).
//!
//! None of `record_compensation_terms`, `declare_prior_employment`,
//! `declare_unsupported_deduction_status` or `record_opening_balance` takes an
//! `EmployerId` of its own — each resolves the Employment's real Employer
//! internally, but none checks it against a caller's authorized one. Every
//! handler here calls [`payroll_app::verify_employment_belongs_to_employer`]
//! first, so an Employment id belonging to another Employer is 404 before any
//! of the four use cases runs.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, Path, State};
use chrono::NaiveDate;
use payroll::{
    EmployerId, EmploymentId, Money, PayPeriod, PriorEmployment, PriorEmploymentFigures, TaxYear,
    UnsupportedDeductionKinds, UnsupportedDeductionStatus,
};
use serde::{Deserialize, Serialize};

use crate::authorized_employer::AuthorizedEmployerContext;
use crate::error::ApiError;
use crate::payroll_error::parse_unsupported_deduction_kind;
use crate::state::AppState;

/// Resolves the caller's authorized `EmployerId` and the path's
/// `EmploymentId`, then confirms the latter belongs to the former — the
/// shared first step of all four handlers below.
async fn authorized_employment(
    state: &AppState,
    context: &AuthorizedEmployerContext,
    employment_id: String,
) -> Result<(EmployerId, EmploymentId), ApiError> {
    let employer_id = EmployerId::new(context.employer_id().as_str());
    let employment_id = EmploymentId::new(employment_id);
    payroll_app::verify_employment_belongs_to_employer(state.db(), &employer_id, &employment_id)
        .await?;
    Ok((employer_id, employment_id))
}

/// Salt's HTTP representation of a pay period. This intentionally belongs to
/// the server rather than serializing `payroll::PayPeriod` directly: the
/// domain type remains free to change without changing the public API.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PayPeriodDto {
    start: NaiveDate,
    end: NaiveDate,
}

impl TryFrom<PayPeriodDto> for PayPeriod {
    type Error = payroll::PayPeriodError;

    fn try_from(period: PayPeriodDto) -> Result<Self, Self::Error> {
        PayPeriod::new(period.start, period.end)
    }
}

impl From<PayPeriod> for PayPeriodDto {
    fn from(period: PayPeriod) -> Self {
        Self {
            start: period.start(),
            end: period.end(),
        }
    }
}

fn parse_pay_periods(periods: Vec<PayPeriodDto>) -> Result<Vec<PayPeriod>, ApiError> {
    periods
        .into_iter()
        .map(PayPeriod::try_from)
        .collect::<Result<_, _>>()
        .map_err(|_| ApiError::malformed_request())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DivergingPeriodsResponse {
    diverging_periods: Vec<PayPeriodDto>,
}

/// The body of a route that records a fact and has nothing to report back:
/// `{}`. A hand-written DTO rather than an ad-hoc `serde_json::json!({})`,
/// so that the day one of these routes does gain a field, the field is added
/// to a named type the rest of this file already returns (§0.32).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordedResponse {}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordCompensationTermsRequest {
    effective_from: NaiveDate,
    basic_pay_cents: i64,
    #[serde(default)]
    acknowledged_diverging_periods: Vec<PayPeriodDto>,
    #[serde(default)]
    reason: String,
}

/// `POST /api/employers/{e}/employments/{em}/compensation-terms`. Wires
/// `record_compensation_terms`, never `correct_compensation_terms` — that use
/// case corrects an existing row and carries the divergence-acknowledgement
/// protocol into a shape this ticket does not own.
pub(crate) async fn record_compensation_terms(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, employment_id)): Path<(String, String)>,
    body: Result<Json<RecordCompensationTermsRequest>, JsonRejection>,
) -> Result<Json<DivergingPeriodsResponse>, ApiError> {
    let Json(request) = body.map_err(|_rejection| ApiError::malformed_request())?;
    let (_employer_id, employment_id) =
        authorized_employment(&state, &context, employment_id).await?;
    let acknowledged_diverging_periods = parse_pay_periods(request.acknowledged_diverging_periods)?;

    let basic_pay =
        Money::from_cents(request.basic_pay_cents).map_err(|_| ApiError::malformed_request())?;

    let diverging_periods = payroll_app::record_compensation_terms(
        state.db(),
        &employment_id,
        request.effective_from,
        basic_pay,
        &acknowledged_diverging_periods,
        &request.reason,
        &context.actor(),
    )
    .await?;

    Ok(Json(DivergingPeriodsResponse {
        diverging_periods: diverging_periods
            .into_iter()
            .map(PayPeriodDto::from)
            .collect(),
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeclarePriorEmploymentRequest {
    tax_year: i32,
    /// `"confirmed_none"` or `"present"` — the same two-valued status
    /// `prior_employment_declaration`'s own `status` column stores.
    /// `PriorEmployment::Unknown` has no wire spelling: nobody ever declares
    /// "unknown", it is only ever what a missing row already means.
    status: String,
    taxable_remuneration_cents: Option<i64>,
    paye_cents: Option<i64>,
}

/// Transport shape only — never a payroll rule. `"present"` needs both
/// figures and `"confirmed_none"` must carry neither: a body stating there
/// was no prior employment *and* naming a prior PAYE amount says two
/// different things, and silently keeping one of them would leave an
/// Operator believing figures were recorded that were not. This is the same
/// judgement `POST /employments` already makes about `personId` and
/// `fullName` together (issue #51).
fn parse_prior_employment(
    request: &DeclarePriorEmploymentRequest,
) -> Result<PriorEmployment, ApiError> {
    match request.status.as_str() {
        "confirmed_none" => {
            if request.taxable_remuneration_cents.is_some() || request.paye_cents.is_some() {
                return Err(ApiError::malformed_request());
            }
            Ok(PriorEmployment::None)
        }
        "present" => {
            let (Some(taxable_remuneration_cents), Some(paye_cents)) =
                (request.taxable_remuneration_cents, request.paye_cents)
            else {
                return Err(ApiError::malformed_request());
            };
            let taxable_remuneration = Money::from_cents(taxable_remuneration_cents)
                .map_err(|_| ApiError::malformed_request())?;
            let paye = Money::from_cents(paye_cents).map_err(|_| ApiError::malformed_request())?;
            Ok(PriorEmployment::Some(PriorEmploymentFigures::new(
                taxable_remuneration,
                paye,
            )))
        }
        _ => Err(ApiError::malformed_request()),
    }
}

/// `POST /api/employers/{e}/employments/{em}/prior-employment`.
pub(crate) async fn declare_prior_employment(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, employment_id)): Path<(String, String)>,
    body: Result<Json<DeclarePriorEmploymentRequest>, JsonRejection>,
) -> Result<Json<RecordedResponse>, ApiError> {
    let Json(request) = body.map_err(|_rejection| ApiError::malformed_request())?;
    let (_employer_id, employment_id) =
        authorized_employment(&state, &context, employment_id).await?;
    let prior_employment = parse_prior_employment(&request)?;

    payroll_app::declare_prior_employment(
        state.db(),
        &employment_id,
        TaxYear::starting(request.tax_year),
        prior_employment,
        &context.actor(),
    )
    .await?;

    Ok(Json(RecordedResponse {}))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeclareUnsupportedDeductionStatusRequest {
    effective_from: NaiveDate,
    /// `"confirmed_none"` or `"present"`, the same two-valued status the
    /// `unsupported_deduction_declaration` table stores. `Unknown` has no
    /// wire spelling, for the same reason as `prior-employment`'s `status`.
    status: String,
    #[serde(default)]
    kinds: Vec<String>,
    #[serde(default)]
    acknowledged_diverging_periods: Vec<PayPeriodDto>,
    #[serde(default)]
    reason: String,
}

/// Transport shape only, and refusing a contradiction for the same reason
/// [`parse_prior_employment`] does: `"confirmed_none"` alongside a non-empty
/// `kinds` claims both that there are no unsupported deductions and that
/// there are these ones. An empty `kinds` under `"present"` is refused by
/// [`UnsupportedDeductionKinds::new`] itself.
fn parse_unsupported_deduction_status(
    request: &DeclareUnsupportedDeductionStatusRequest,
) -> Result<UnsupportedDeductionStatus, ApiError> {
    match request.status.as_str() {
        "confirmed_none" => {
            if !request.kinds.is_empty() {
                return Err(ApiError::malformed_request());
            }
            Ok(UnsupportedDeductionStatus::ConfirmedNone)
        }
        "present" => {
            let mut kinds = Vec::with_capacity(request.kinds.len());
            for code in &request.kinds {
                kinds.push(
                    parse_unsupported_deduction_kind(code)
                        .ok_or_else(ApiError::malformed_request)?,
                );
            }
            let kinds =
                UnsupportedDeductionKinds::new(kinds).map_err(|_| ApiError::malformed_request())?;
            Ok(UnsupportedDeductionStatus::Present(kinds))
        }
        _ => Err(ApiError::malformed_request()),
    }
}

/// `POST /api/employers/{e}/employments/{em}/unsupported-deductions`.
pub(crate) async fn declare_unsupported_deduction_status(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, employment_id)): Path<(String, String)>,
    body: Result<Json<DeclareUnsupportedDeductionStatusRequest>, JsonRejection>,
) -> Result<Json<DivergingPeriodsResponse>, ApiError> {
    let Json(request) = body.map_err(|_rejection| ApiError::malformed_request())?;
    let (_employer_id, employment_id) =
        authorized_employment(&state, &context, employment_id).await?;
    let status = parse_unsupported_deduction_status(&request)?;
    let acknowledged_diverging_periods = parse_pay_periods(request.acknowledged_diverging_periods)?;

    let diverging_periods = payroll_app::declare_unsupported_deduction_status(
        state.db(),
        &employment_id,
        request.effective_from,
        status,
        &acknowledged_diverging_periods,
        &request.reason,
        &context.actor(),
    )
    .await?;

    Ok(Json(DivergingPeriodsResponse {
        diverging_periods: diverging_periods
            .into_iter()
            .map(PayPeriodDto::from)
            .collect(),
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordOpeningBalanceRequest {
    tax_year: i32,
    salt_coverage_start: NaiveDate,
    prior_taxable_remuneration_cents: i64,
    prior_paye_cents: i64,
}

/// `POST /api/employers/{e}/employments/{em}/opening-balance`.
pub(crate) async fn record_opening_balance(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, employment_id)): Path<(String, String)>,
    body: Result<Json<RecordOpeningBalanceRequest>, JsonRejection>,
) -> Result<Json<RecordedResponse>, ApiError> {
    let Json(request) = body.map_err(|_rejection| ApiError::malformed_request())?;
    let (_employer_id, employment_id) =
        authorized_employment(&state, &context, employment_id).await?;

    let prior_taxable_remuneration = Money::from_cents(request.prior_taxable_remuneration_cents)
        .map_err(|_| ApiError::malformed_request())?;
    let prior_paye =
        Money::from_cents(request.prior_paye_cents).map_err(|_| ApiError::malformed_request())?;

    payroll_app::record_opening_balance(
        state.db(),
        &employment_id,
        TaxYear::starting(request.tax_year),
        request.salt_coverage_start,
        prior_taxable_remuneration,
        prior_paye,
        &context.actor(),
    )
    .await?;

    Ok(Json(RecordedResponse {}))
}
