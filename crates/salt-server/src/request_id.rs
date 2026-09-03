//! The per-request id issue #45 requires in every log line, and in
//! `details.requestId` on a 500 response only.
//!
//! Held as a [`tokio::task_local`] rather than passed as a handler argument:
//! [`crate::error::ApiError::internal`] is called from ordinary handler code
//! and from `tower_http`'s panic handler alike, and the panic handler is
//! handed only the panic payload — never the request. A task-local is
//! readable from both, because [`middleware`] wraps the whole request,
//! including the panic-catching layer beneath it, in one [`scope`] call.
//!
//! [`scope`]: tokio::task_local::LocalKey::scope

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument;
use uuid::Uuid;

tokio::task_local! {
    pub(crate) static CURRENT_REQUEST_ID: String;
}

/// The current request's id, or `"unknown"` outside [`middleware`]'s scope —
/// which every request passes through, so this only reads as `"unknown"` in
/// a test that calls [`crate::error::ApiError::internal`] without first
/// entering a [`CURRENT_REQUEST_ID`] scope itself.
pub(crate) fn current_request_id() -> String {
    CURRENT_REQUEST_ID
        .try_with(Clone::clone)
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Generates a request id, opens a tracing span carrying it so every log
/// line emitted while handling this request names it, and holds it in a
/// task-local for [`current_request_id`] to read back — including from
/// inside a handler panic, since the panic is caught by a layer nested
/// inside this one (see [`crate::router::build_router`]'s layer ordering).
///
/// Logs one line per request itself, rather than leaving that to whatever a
/// handler happens to log: a healthy `GET /api/health` calls
/// [`tracing::info`] or [`tracing::error`] nowhere in its own code, and
/// issue #45 asks for a request id "in every log line", not only in the
/// lines a future handler happens to add.
pub(crate) async fn middleware(request: Request, next: Next) -> Response {
    let request_id = Uuid::new_v4().to_string();
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let span = tracing::info_span!(
        "request",
        request_id = %request_id,
        method = %method,
        path = %path,
    );

    CURRENT_REQUEST_ID
        .scope(request_id, async move {
            let started_at = std::time::Instant::now();
            let response = next.run(request).await;
            tracing::info!(
                status = response.status().as_u16(),
                elapsed_ms = started_at.elapsed().as_millis() as u64,
                "request completed"
            );
            response
        })
        .instrument(span)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn outside_any_scope_the_request_id_is_reported_as_unknown() {
        assert_eq!(current_request_id(), "unknown");
    }

    #[tokio::test]
    async fn inside_a_scope_the_request_id_is_the_scoped_value() {
        let seen = CURRENT_REQUEST_ID
            .scope("abc-123".to_string(), async { current_request_id() })
            .await;

        assert_eq!(seen, "abc-123");
    }
}
