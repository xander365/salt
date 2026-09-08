//! `GET`/`PUT /api/employers/{e}/particulars` (issue #71, parent #70 D-7).
//! The Employer's own screen: any active member reads `EmployerParticulars`,
//! only an Owner may write it — this is the first Owner-only route
//! [`AuthorizedEmployerContext::require_role`] gates (§0.6).
//!
//! `PUT` covers both "record" and "correct": `payroll_app::set_employer_particulars`
//! decides which this write is, since the resource carries no `effective_from`
//! of its own to split the two acts by (see that function's own docs). Every
//! handler here takes its `EmployerId` from [`AuthorizedEmployerContext`] and
//! never from the path (ADR-0017), and the actor is always the authorized
//! context's own, never the request body (ADR-0019).

use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, State};
use chrono::{DateTime, Utc};
use payroll::EmployerId;
use payroll_app::{EmployerParticulars, EmployerParticularsFields, MembershipRole};
use serde::{Deserialize, Serialize};

use crate::authorized_employer::AuthorizedEmployerContext;
use crate::employment_facts::{DivergingPeriodsResponse, PayPeriodDto, parse_pay_periods};
use crate::error::ApiError;
use crate::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EmployerParticularsResponse {
    registered_name: String,
    address_line1: String,
    address_line2: Option<String>,
    city: String,
    postal_code: Option<String>,
    income_tax_number: Option<String>,
    social_security_number: Option<String>,
    created_at: DateTime<Utc>,
    created_by: String,
}

impl From<EmployerParticulars> for EmployerParticularsResponse {
    fn from(particulars: EmployerParticulars) -> Self {
        Self {
            registered_name: particulars.registered_name,
            address_line1: particulars.address_line1,
            address_line2: particulars.address_line2,
            city: particulars.city,
            postal_code: particulars.postal_code,
            income_tax_number: particulars.income_tax_number,
            social_security_number: particulars.social_security_number,
            created_at: particulars.created_at,
            created_by: particulars.created_by,
        }
    }
}

/// `GET /api/employers/{e}/particulars`. Any active member reads this —
/// Owner and PayrollOperator alike — so this handler calls
/// [`AuthorizedEmployerContext::require_role`] with neither: reaching this
/// far already proves membership (§0.6, "Everything payroll is
/// PayrollOperator or above"). `null` when this Employer has never recorded
/// particulars yet, never a 404 — the Employer itself is what the URL names,
/// and it exists.
pub(crate) async fn get_employer_particulars(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
) -> Result<Json<Option<EmployerParticularsResponse>>, ApiError> {
    let employer_id = EmployerId::new(context.employer_id().as_str());

    let particulars = payroll_app::get_employer_particulars(state.db(), &employer_id).await?;

    Ok(Json(particulars.map(EmployerParticularsResponse::from)))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetEmployerParticularsRequest {
    registered_name: String,
    address_line1: String,
    #[serde(default)]
    address_line2: Option<String>,
    city: String,
    #[serde(default)]
    postal_code: Option<String>,
    #[serde(default)]
    income_tax_number: Option<String>,
    #[serde(default)]
    social_security_number: Option<String>,
    #[serde(default)]
    acknowledged_diverging_periods: Vec<PayPeriodDto>,
    #[serde(default)]
    reason: String,
}

/// A blank optional field is sent as `None` rather than as `Some("")`: the
/// wire has no way to distinguish "not provided" from "provided empty", and
/// the domain layer's own not-blank-if-present guard exists for a value that
/// arrives non-empty but whitespace-only, not for this.
fn blank_to_none(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

/// `PUT /api/employers/{e}/particulars`. Owner-only — issue #71's own first
/// Owner-only route.
pub(crate) async fn set_employer_particulars(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    body: Result<Json<SetEmployerParticularsRequest>, JsonRejection>,
) -> Result<Json<DivergingPeriodsResponse>, ApiError> {
    context.require_role(MembershipRole::Owner)?;
    let Json(request) = body.map_err(|_rejection| ApiError::malformed_request())?;
    let employer_id = EmployerId::new(context.employer_id().as_str());

    let acknowledged_diverging_periods = parse_pay_periods(request.acknowledged_diverging_periods)?;

    let fields = EmployerParticularsFields {
        registered_name: request.registered_name,
        address_line1: request.address_line1,
        address_line2: blank_to_none(request.address_line2),
        city: request.city,
        postal_code: blank_to_none(request.postal_code),
        income_tax_number: blank_to_none(request.income_tax_number),
        social_security_number: blank_to_none(request.social_security_number),
    };

    let diverging_periods = payroll_app::set_employer_particulars(
        state.db(),
        &employer_id,
        fields,
        &acknowledged_diverging_periods,
        &request.reason,
        &context.actor(),
    )
    .await?;

    Ok(Json(DivergingPeriodsResponse::from_periods(
        diverging_periods,
    )))
}
