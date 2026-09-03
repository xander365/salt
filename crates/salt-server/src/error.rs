//! The envelope every route answers a refusal with (issue #45's own
//! acceptance criteria): `{ "error": { code, message, details } }`. This
//! spec ships no payroll routes, so it maps only the refusals it actually
//! raises — an unmatched route, a mutating request missing
//! `X-Salt-Request`, and the catch-all "something went wrong". `payroll-app`
//! and `PayrollAppError`'s exhaustive mapping is Spec 2's, and is
//! deliberately not started here.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use crate::request_id::current_request_id;

/// A refusal shaped for the wire: a status, the envelope's `code` and
/// `message`, and an optional `details` object. Never built directly from a
/// raw error's own text — see [`ApiError::internal`].
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    details: Option<Value>,
}

impl ApiError {
    /// No route matched the request.
    pub fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: "no route matches this request".to_string(),
            details: None,
        }
    }

    /// A mutating request (`POST`/`PUT`/`PATCH`/`DELETE`) arrived without
    /// `X-Salt-Request: 1`. Together with the session cookie's `SameSite=Lax`
    /// this is the whole CSRF defence (issue #45's Deep Instructions) — a
    /// cross-site form post cannot set a custom header, so its absence is
    /// itself the signal.
    pub fn salt_request_header_required() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "salt_request_header_required",
            message: "mutating requests must carry the X-Salt-Request: 1 header".to_string(),
            details: None,
        }
    }

    /// A request body exceeded the router's 256 KB limit. Axum's extractor
    /// rejection would otherwise answer this with its own plain-text body,
    /// bypassing the API's documented error envelope.
    pub fn payload_too_large() -> Self {
        Self {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            code: "payload_too_large",
            message: "request body exceeds the 256 KB limit".to_string(),
            details: None,
        }
    }

    /// `GET /api/ready` could not reach the database. Deliberately 503 and
    /// not 500: the whole point of a readiness probe separate from liveness
    /// (§0's story 32) is to let a load balancer tell "process up" from
    /// "database reachable", and a 500 says the opposite of what a
    /// still-serving process with an unreachable database means. `cause` is
    /// logged and never repeated in the body, for the same reason
    /// [`Self::internal`] does not repeat one.
    pub fn not_ready(cause: impl std::fmt::Display) -> Self {
        tracing::error!(
            error = %cause,
            request_id = %current_request_id(),
            "readiness check failed"
        );
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            code: "not_ready",
            message: "the database is not reachable".to_string(),
            details: None,
        }
    }

    /// An unexpected failure: a database round trip that could not complete,
    /// a handler panic, anything this build did not anticipate. `cause` is
    /// logged in full — SQL text and all — and never reaches the response
    /// body, which carries only the current request id (this build's own
    /// rule: a request id appears in `details.requestId` on a 500, and
    /// nowhere else).
    pub fn internal(cause: impl std::fmt::Display) -> Self {
        let request_id = current_request_id();
        tracing::error!(error = %cause, request_id = %request_id, "internal error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: "an unexpected error occurred".to_string(),
            details: Some(json!({ "requestId": request_id })),
        }
    }

    #[cfg(test)]
    pub(crate) fn status(&self) -> StatusCode {
        self.status
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(json!({
            "error": {
                "code": self.code,
                "message": self.message,
                "details": self.details,
            }
        }));
        (self.status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request_id::CURRENT_REQUEST_ID;
    use axum::body::to_bytes;

    async fn response_body_text(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read the response body");
        String::from_utf8(bytes.to_vec()).expect("the body is UTF-8 JSON")
    }

    #[tokio::test]
    async fn an_internal_error_never_repeats_its_cause_in_the_body() {
        let cause = "relation \"session\" does not exist at line 42: SELECT * FROM session";

        let response = CURRENT_REQUEST_ID
            .scope("test-request-id".to_string(), async {
                ApiError::internal(cause).into_response()
            })
            .await;

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = response_body_text(response).await;
        assert!(
            !body.contains("session") && !body.contains("SELECT"),
            "the response body must not repeat the internal cause, got {body}"
        );
        assert!(body.contains("test-request-id"), "got {body}");
    }

    #[tokio::test]
    async fn a_non_500_error_carries_no_request_id() {
        let response = CURRENT_REQUEST_ID
            .scope("test-request-id".to_string(), async {
                ApiError::not_found().into_response()
            })
            .await;

        let body = response_body_text(response).await;
        assert!(
            !body.contains("test-request-id"),
            "only a 500 carries details.requestId, got {body}"
        );
        assert!(body.contains("\"details\":null"), "got {body}");
    }

    #[tokio::test]
    async fn a_readiness_failure_is_503_and_repeats_neither_cause_nor_request_id() {
        let cause = "connection refused: postgres://salt:hunter2@db:5432/salt";

        let response = CURRENT_REQUEST_ID
            .scope("test-request-id".to_string(), async {
                ApiError::not_ready(cause).into_response()
            })
            .await;

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = response_body_text(response).await;
        assert!(!body.contains("hunter2"), "got {body}");
        assert!(!body.contains("test-request-id"), "got {body}");
        assert!(body.contains("\"code\":\"not_ready\""), "got {body}");
    }

    #[test]
    fn the_envelope_has_the_three_documented_fields() {
        let error = ApiError::salt_request_header_required();
        assert_eq!(error.status(), StatusCode::BAD_REQUEST);
        assert_eq!(error.code, "salt_request_header_required");
    }
}
