//! `GET /api/employers` (issue #47, parent #38 Spec 1 of 3; §0.22, §0.30).
//! The authenticated Operator's own Employers — every active
//! `EmployerMembership` they hold, named with the Employer's own `name`
//! (issue #40) rather than only an id.
//!
//! Session-scoped, not Employer-scoped: there is no Employer in this route's
//! URL to authorize against, so it reads the same two facts
//! [`crate::session::who_am_i`] does — a valid session and an active
//! Operator, through [`crate::session::authenticated_operator`] — and never
//! reaches for `AuthorizedEmployerContext`, which exists for routes that
//! name one particular Employer.

use axum::extract::{Json, State};
use axum::http::HeaderMap;
use chrono::Utc;
use serde::Serialize;

use crate::error::ApiError;
use crate::session::{RoleDto, authenticated_operator};
use crate::state::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EmployersResponse {
    employers: Vec<EmployerDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EmployerDto {
    employer_id: String,
    name: String,
    role: RoleDto,
}

pub(crate) async fn list_employers(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<EmployersResponse>, ApiError> {
    let operator = authenticated_operator(&state, &headers, Utc::now()).await?;

    let employers = payroll_app::list_employers_for_operator(state.db(), &operator.id)
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(|summary| EmployerDto {
            employer_id: summary.id.to_string(),
            name: summary.name,
            role: summary.role.into(),
        })
        .collect();

    Ok(Json(EmployersResponse { employers }))
}
