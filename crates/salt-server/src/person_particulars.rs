//! `GET`/`PUT /api/employers/{e}/people/{p}/particulars` and `PUT
//! /api/employers/{e}/people/{p}/name` (issue #72, parent #70 D-7). The
//! correction trail is not a route of its own: it rides on the particulars
//! `GET`, because the screen that shows it never wants one without the
//! other, and a second round trip would only let the two disagree.
//!
//! Unlike `employer_particulars.rs`, no handler here calls `require_role`:
//! PersonParticulars are not Owner-only (D25 restricts only Employer
//! particulars), so any active member of the Employer reads and writes
//! everything on this file.
//!
//! Every handler takes its `EmployerId` from [`AuthorizedEmployerContext`]
//! and the `PersonId` from the path, and every `payroll_app` call here takes
//! both — the person-belongs-to-employer check happens inside that call, not
//! as a separate lookup this file would have to remember to make first
//! (ADR-0017).

use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, Path, State};
use chrono::{DateTime, Utc};
use payroll::{EmployerId, PersonId};
use payroll_app::{PersonParticulars, PersonParticularsFields};
use serde::{Deserialize, Serialize};

use crate::authorized_employer::AuthorizedEmployerContext;
use crate::employment_facts::{DivergingPeriodsResponse, PayPeriodDto, parse_pay_periods};
use crate::error::ApiError;
use crate::state::AppState;

fn authorized_employer_and_person(
    context: &AuthorizedEmployerContext,
    person_id: String,
) -> (EmployerId, PersonId) {
    (
        EmployerId::new(context.employer_id().as_str()),
        PersonId::new(person_id),
    )
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ActionLogEntryResponse {
    occurred_at: DateTime<Utc>,
    actor: String,
    action_type: String,
    context: Option<serde_json::Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PersonParticularsResponse {
    full_name: String,
    identity_number: Option<String>,
    address_line1: Option<String>,
    address_line2: Option<String>,
    city: Option<String>,
    postal_code: Option<String>,
    particulars_created_at: Option<DateTime<Utc>>,
    particulars_created_by: Option<String>,
    action_log: Vec<ActionLogEntryResponse>,
}

impl PersonParticularsResponse {
    fn from_parts(
        particulars: PersonParticulars,
        action_log: Vec<payroll_app::ActionLogEntryRecord>,
    ) -> Self {
        Self {
            full_name: particulars.full_name,
            identity_number: particulars.identity_number,
            address_line1: particulars.address_line1,
            address_line2: particulars.address_line2,
            city: particulars.city,
            postal_code: particulars.postal_code,
            particulars_created_at: particulars.particulars_created_at,
            particulars_created_by: particulars.particulars_created_by,
            action_log: action_log
                .into_iter()
                .map(|entry| ActionLogEntryResponse {
                    occurred_at: entry.occurred_at,
                    actor: entry.actor,
                    action_type: entry.action_type,
                    context: entry.context,
                })
                .collect(),
        }
    }
}

/// `GET /api/employers/{e}/people/{p}/particulars`. Any active member reads
/// this — the Employment screen's own "who is this and what does Salt have
/// on record for them" panel, `fullName` and `PersonParticulars` together,
/// plus every ActionLog entry either one has ever produced (issue #72's own
/// "the Employment screen shows that trail").
pub(crate) async fn get_person_particulars(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, person_id)): Path<(String, String)>,
) -> Result<Json<PersonParticularsResponse>, ApiError> {
    let (employer_id, person_id) = authorized_employer_and_person(&context, person_id);

    let particulars =
        payroll_app::get_person_particulars(state.db(), &employer_id, &person_id).await?;
    let action_log = payroll_app::list_action_log_entries_for_target(
        state.db(),
        &employer_id,
        "person",
        person_id.as_str(),
    )
    .await?;

    Ok(Json(PersonParticularsResponse::from_parts(
        particulars,
        action_log,
    )))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetPersonParticularsRequest {
    identity_number: String,
    address_line1: String,
    #[serde(default)]
    address_line2: Option<String>,
    city: String,
    #[serde(default)]
    postal_code: Option<String>,
    #[serde(default)]
    acknowledged_diverging_periods: Vec<PayPeriodDto>,
    #[serde(default)]
    reason: String,
}

/// `PUT /api/employers/{e}/people/{p}/particulars`.
pub(crate) async fn set_person_particulars(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, person_id)): Path<(String, String)>,
    body: Result<Json<SetPersonParticularsRequest>, JsonRejection>,
) -> Result<Json<DivergingPeriodsResponse>, ApiError> {
    let Json(request) = body.map_err(|_rejection| ApiError::malformed_request())?;
    let (employer_id, person_id) = authorized_employer_and_person(&context, person_id);
    let acknowledged_diverging_periods = parse_pay_periods(request.acknowledged_diverging_periods)?;

    let fields = PersonParticularsFields {
        identity_number: request.identity_number,
        address_line1: request.address_line1,
        address_line2: request.address_line2,
        city: request.city,
        postal_code: request.postal_code,
    };

    let diverging_periods = payroll_app::set_person_particulars(
        state.db(),
        &employer_id,
        &person_id,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CorrectPersonFullNameRequest {
    full_name: String,
    #[serde(default)]
    acknowledged_diverging_periods: Vec<PayPeriodDto>,
    #[serde(default)]
    reason: String,
}

/// `PUT /api/employers/{e}/people/{p}/name`. Corrects a misspelled
/// `full_name` — the write `person`'s own migration made impossible until
/// this ticket restored a column-scoped `UPDATE` grant.
pub(crate) async fn correct_person_full_name(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, person_id)): Path<(String, String)>,
    body: Result<Json<CorrectPersonFullNameRequest>, JsonRejection>,
) -> Result<Json<DivergingPeriodsResponse>, ApiError> {
    let Json(request) = body.map_err(|_rejection| ApiError::malformed_request())?;
    let (employer_id, person_id) = authorized_employer_and_person(&context, person_id);
    let acknowledged_diverging_periods = parse_pay_periods(request.acknowledged_diverging_periods)?;

    let diverging_periods = payroll_app::correct_person_full_name(
        state.db(),
        &employer_id,
        &person_id,
        &request.full_name,
        &acknowledged_diverging_periods,
        &request.reason,
        &context.actor(),
    )
    .await?;

    Ok(Json(DivergingPeriodsResponse::from_periods(
        diverging_periods,
    )))
}
