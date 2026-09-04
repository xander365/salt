//! `POST /api/employers/{e}/employments`, `GET /api/employers/{e}/employments`
//! and `GET /api/employers/{e}/employments/{em}` (issue #51, parent #49 Spec
//! 2 of 3; ADR-0020). An Operator adds an employee by typing their name, and
//! `payroll_app::create_employment` creates the Person and the Employment
//! together, in one transaction — there is no separate Person creation or
//! Person list route, because a Person exists only as that side effect.
//!
//! Every handler here takes its `EmployerId` from [`AuthorizedEmployerContext`]
//! and never from the path (ADR-0017): the extractor is the only way a
//! handler can obtain one.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, Path, State};
use chrono::{NaiveDate, Utc};
use payroll::{EmployerId, EmploymentId, PersonId};
use payroll_app::EmploymentPerson;
use serde::{Deserialize, Serialize};

use crate::authorized_employer::AuthorizedEmployerContext;
use crate::error::ApiError;
use crate::state::AppState;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateEmploymentRequest {
    person_id: Option<String>,
    full_name: Option<String>,
    start_date: NaiveDate,
    end_date: Option<NaiveDate>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateEmploymentResponse {
    employment_id: String,
    person_id: String,
}

/// `POST /api/employers/{e}/employments`: accepts exactly one of `personId`
/// or `fullName` — both, or neither, is 400 `malformed_request`, decided
/// here rather than by `payroll_app`, which never sees an ambiguous pair.
pub(crate) async fn create_employment(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    body: Result<Json<CreateEmploymentRequest>, JsonRejection>,
) -> Result<Json<CreateEmploymentResponse>, ApiError> {
    let Json(request) = body.map_err(|_rejection| ApiError::malformed_request())?;

    let person = match (request.person_id, request.full_name) {
        (Some(person_id), None) => EmploymentPerson::Existing(PersonId::new(person_id)),
        (None, Some(full_name)) => EmploymentPerson::New(full_name),
        _ => return Err(ApiError::malformed_request()),
    };

    let employer_id = EmployerId::new(context.employer_id().as_str());

    let (person_id, employment_id) = payroll_app::create_employment(
        state.db(),
        &employer_id,
        person,
        request.start_date,
        request.end_date,
        &context.actor(),
    )
    .await?;

    Ok(Json(CreateEmploymentResponse {
        employment_id: employment_id.to_string(),
        person_id: person_id.to_string(),
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EmploymentsResponse {
    employments: Vec<EmploymentListingDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EmploymentListingDto {
    employment_id: String,
    person_id: String,
    full_name: String,
}

/// `GET /api/employers/{e}/employments`: every Employment this Employer
/// has, named with its Person's `fullName` beside the ids. No pagination,
/// no filters, no sorting (issue #51's own Deep Instructions).
pub(crate) async fn list_employments(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
) -> Result<Json<EmploymentsResponse>, ApiError> {
    let employer_id = EmployerId::new(context.employer_id().as_str());

    let employments = payroll_app::list_employments_for_employer(state.db(), &employer_id)
        .await?
        .into_iter()
        .map(|listing| EmploymentListingDto {
            employment_id: listing.id.to_string(),
            person_id: listing.person_id.to_string(),
            full_name: listing.full_name,
        })
        .collect();

    Ok(Json(EmploymentsResponse { employments }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EmploymentDetailResponse {
    employment_id: String,
    person_id: String,
    full_name: String,
    start_date: NaiveDate,
    end_date: Option<NaiveDate>,
    current_basic_pay_cents: Option<i64>,
}

/// `GET /api/employers/{e}/employments/{em}`: the Employment's dates, its
/// Person's `fullName` and its current pay — the `CompensationTerms` row in
/// force today, if any. An Employment id belonging to another Employer is
/// 404, indistinguishable from one that does not exist (ADR-0017).
pub(crate) async fn get_employment(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, employment_id)): Path<(String, String)>,
) -> Result<Json<EmploymentDetailResponse>, ApiError> {
    let employer_id = EmployerId::new(context.employer_id().as_str());
    let employment_id = EmploymentId::new(employment_id);

    let detail = payroll_app::get_employment_detail(
        state.db(),
        &employer_id,
        &employment_id,
        Utc::now().date_naive(),
    )
    .await?;

    Ok(Json(EmploymentDetailResponse {
        employment_id: detail.id.to_string(),
        person_id: detail.person_id.to_string(),
        full_name: detail.full_name,
        start_date: detail.start_date,
        end_date: detail.end_date,
        current_basic_pay_cents: detail.current_basic_pay.map(|money| money.cents()),
    }))
}
