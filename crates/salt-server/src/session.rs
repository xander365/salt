//! `POST /api/session` (login), `DELETE /api/session` (logout) and `GET
//! /api/session` (who am I) — issue #46, parent #38. The whole surface sits
//! on `payroll_app`'s Operator and session use cases (ADR-0018: this crate
//! writes no SQL of its own); this module owns only the HTTP shape —
//! parsing the request, building the `Set-Cookie` header, and mapping
//! refusals to the documented envelope.
//!
//! Every kind of login failure looks the same from outside (issue #46's own
//! acceptance criterion): [`login`] maps every
//! [`payroll_app::PayrollAppError::OperatorCredentialInvalid`] to the
//! identical 401 `invalid_credentials`, carrying nothing a caller could use
//! to tell a wrong password apart from an unknown email, a disabled
//! Operator or a locked account — `payroll_app::verify_operator_credential`
//! already made that decision; this module only forwards it.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Json, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use payroll_app::{MembershipRole, MembershipStatus, OperatorStatus, PayrollAppError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::ApiError;
use crate::state::AppState;

/// The cookie every login sets and every logout clears. Its name is not
/// itself part of the documented contract — §0.33 makes `GET /api/session`
/// the source of truth about being signed in, not the cookie's own name —
/// only the three routes below ever read or write it.
const SESSION_COOKIE_NAME: &str = "salt_session";

#[derive(Deserialize)]
pub(crate) struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionResponse {
    operator: OperatorDto,
    memberships: Vec<MembershipDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OperatorDto {
    id: String,
    email: String,
    display_name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MembershipDto {
    employer_id: String,
    role: RoleDto,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
enum RoleDto {
    Owner,
    PayrollOperator,
}

impl From<MembershipRole> for RoleDto {
    fn from(role: MembershipRole) -> Self {
        match role {
            MembershipRole::Owner => Self::Owner,
            MembershipRole::PayrollOperator => Self::PayrollOperator,
        }
    }
}

/// `POST /api/session`: verifies the credential, mints a fresh session and
/// sets its cookie. The response body carries no Operator state at all —
/// §0.33's own instruction is that `GET /api/session` is the *only* source
/// of truth about being signed in, so a client that wants the Operator or
/// their memberships makes that call next, the same way [`logout`]'s own
/// response carries none either.
pub(crate) async fn login(
    State(state): State<AppState>,
    body: Result<Json<LoginRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(LoginRequest { email, password }) =
        body.map_err(|_rejection| ApiError::malformed_request())?;
    let now = Utc::now();

    let operator_id = payroll_app::verify_operator_credential(state.db(), &email, &password, now)
        .await
        .map_err(|err| match err {
            PayrollAppError::OperatorCredentialInvalid => ApiError::invalid_credentials(),
            other => ApiError::internal(other),
        })?;

    // Minting the new row and clearing that Operator's already-expired ones
    // are one use case (§0.12: "the token rotates on login, and that
    // Operator's other expired session rows are cleared at login"), so this
    // handler cannot perform the first without the second.
    let created = payroll_app::create_session_for_active_operator(state.db(), &operator_id, now)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::invalid_credentials)?;

    let mut response = ok_body().into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        session_cookie(&created.token, state.secure_cookies()),
    );
    Ok(response)
}

/// `DELETE /api/session`: deletes the session the cookie names, if any, and
/// always clears the cookie. Idempotent, the same discipline
/// [`payroll_app::delete_session`] itself applies: signing out with no
/// cookie, or one that no longer names a live session, reaches the same end
/// state as signing out of a live one, so none of those is refused.
pub(crate) async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if let Some(token) = session_token(&headers) {
        let snapshot = payroll_app::load_session(state.db(), token, Utc::now())
            .await
            .map_err(ApiError::internal)?;
        if let Some(snapshot) = snapshot {
            payroll_app::delete_session(state.db(), &snapshot.id)
                .await
                .map_err(ApiError::internal)?;
        }
    }

    let mut response = ok_body().into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        expired_session_cookie(state.secure_cookies()),
    );
    Ok(response)
}

/// `GET /api/session`: the only source of truth about being signed in
/// (§0.33). A missing cookie, a token naming no row, and an expired session
/// all answer the identical 401 `unauthenticated` — nothing here needs to
/// tell them apart, any more than [`login`] needs to explain a login
/// failure.
pub(crate) async fn who_am_i(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SessionResponse>, ApiError> {
    let token = session_token(&headers).ok_or_else(ApiError::unauthenticated)?;

    let snapshot = payroll_app::load_session(state.db(), token, Utc::now())
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(ApiError::unauthenticated)?;

    let operator = payroll_app::find_operator_by_id(state.db(), &snapshot.operator_id)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| {
            ApiError::internal("a live session names an Operator that no longer exists")
        })?;

    // A live session is not on its own proof of being signed in: §0's own
    // pipeline reads "session valid, Operator active", and story 18 asks a
    // disabled Operator to stop working immediately rather than when their
    // session happens to expire. Read fresh here, on every request, which is
    // the whole reason disabling needs no cache invalidation — and refused
    // with the identical `unauthenticated` a missing cookie answers, so this
    // route says no more about why than [`login`] does.
    if operator.status != OperatorStatus::Active {
        return Err(ApiError::unauthenticated());
    }

    // Only active memberships: a revoked one grants nothing (ADR-0017), and
    // this response exists so a client can offer the Employers this
    // Operator can actually enter, not an audit trail of ones they once
    // could.
    let memberships = payroll_app::list_employer_memberships(state.db(), &snapshot.operator_id)
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .filter(|membership| membership.status == MembershipStatus::Active)
        .map(|membership| MembershipDto {
            employer_id: membership.employer_id.to_string(),
            role: membership.role.into(),
        })
        .collect();

    Ok(Json(SessionResponse {
        operator: OperatorDto {
            id: operator.id.to_string(),
            email: operator.email,
            display_name: operator.display_name,
        },
        memberships,
    }))
}

/// The minimal, stateless body [`login`] and [`logout`] answer with —
/// [`crate::router::health`] and [`crate::router::ready`]'s own convention.
/// There is nothing else to say: the cookie carries the fact that matters,
/// and §0.33 reserves Operator and membership state for [`who_am_i`] alone.
fn ok_body() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

/// The `Set-Cookie` value a successful login answers with: `HttpOnly`,
/// `SameSite=Lax`, `Path=/`, no `Domain` (issue #46's own acceptance
/// criterion), and `Secure` unless `secure` says otherwise.
fn session_cookie(token: &str, secure: bool) -> HeaderValue {
    cookie_header(token, secure, None)
}

/// The `Set-Cookie` value logout answers with: an empty value and
/// `Max-Age=0`, which every browser reads as "delete this cookie now" —
/// carrying the same `HttpOnly`/`SameSite`/`Path` attributes as the cookie it
/// replaces, so this is an instruction to delete the same cookie rather than
/// a second, different one set under the same name.
fn expired_session_cookie(secure: bool) -> HeaderValue {
    cookie_header("", secure, Some(0))
}

fn cookie_header(value: &str, secure: bool, max_age: Option<u32>) -> HeaderValue {
    let mut header = format!("{SESSION_COOKIE_NAME}={value}; HttpOnly; SameSite=Lax; Path=/");
    if secure {
        header.push_str("; Secure");
    }
    if let Some(max_age) = max_age {
        header.push_str(&format!("; Max-Age={max_age}"));
    }
    HeaderValue::from_str(&header)
        .expect("a cookie built from a token and this module's own fixed text is always valid")
}

/// The session token carried in the `Cookie` request header, or `None` when
/// there is no `Cookie` header, it is not valid UTF-8, or it carries no
/// [`SESSION_COOKIE_NAME`] pair. A browser folds every cookie for the origin
/// into one `; `-separated header (RFC 6265 §5.4) — there is no per-cookie
/// header to read instead.
fn session_token(headers: &HeaderMap) -> Option<&str> {
    let header = headers.get(header::COOKIE)?.to_str().ok()?;
    header.split(';').find_map(|pair| {
        let (name, value) = pair.trim().split_once('=')?;
        (name == SESSION_COOKIE_NAME).then_some(value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header_str(value: &HeaderValue) -> &str {
        value.to_str().unwrap()
    }

    #[test]
    fn a_secure_session_cookie_carries_every_documented_attribute() {
        let header = session_cookie("the-token", true);

        let text = header_str(&header);
        assert!(text.starts_with("salt_session=the-token;"), "{text}");
        assert!(text.contains("HttpOnly"), "{text}");
        assert!(text.contains("SameSite=Lax"), "{text}");
        assert!(text.contains("Path=/"), "{text}");
        assert!(text.contains("Secure"), "{text}");
        assert!(!text.contains("Domain"), "{text}");
    }

    #[test]
    fn an_insecure_session_cookie_omits_only_secure() {
        let header = session_cookie("the-token", false);

        let text = header_str(&header);
        assert!(text.contains("HttpOnly"), "{text}");
        assert!(text.contains("SameSite=Lax"), "{text}");
        assert!(text.contains("Path=/"), "{text}");
        assert!(!text.contains("Secure"), "{text}");
    }

    #[test]
    fn the_expired_cookie_clears_the_value_and_carries_max_age_zero() {
        let header = expired_session_cookie(true);

        let text = header_str(&header);
        assert!(text.starts_with("salt_session=;"), "{text}");
        assert!(text.contains("Max-Age=0"), "{text}");
    }

    #[test]
    fn the_session_token_is_read_from_among_several_cookies() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("other=1; salt_session=abc123; another=2"),
        );

        assert_eq!(session_token(&headers), Some("abc123"));
    }

    #[test]
    fn a_missing_cookie_header_yields_no_token() {
        let headers = HeaderMap::new();

        assert_eq!(session_token(&headers), None);
    }

    #[test]
    fn a_cookie_header_without_the_session_cookie_yields_no_token() {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, HeaderValue::from_static("other=1"));

        assert_eq!(session_token(&headers), None);
    }
}
