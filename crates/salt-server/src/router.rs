//! The real router — the one thing this crate's tests build and drive with
//! `tower::ServiceExt::oneshot`, and the one thing the binary serves. Every
//! transport rule issue #45 names is a layer here, applied once, so no
//! future route can be added without inheriting all of them.

use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use tower_http::catch_panic::CatchPanicLayer;

use crate::employers;
use crate::employments;
use crate::error::ApiError;
use crate::request_id;
use crate::session;
use crate::state::AppState;

/// §0.19's own number: bodies are capped at 256 KB.
const MAX_BODY_BYTES: usize = 256 * 1024;

/// Builds the real router. `salt-server`'s binary calls this once at
/// startup; this crate's own tests call it to get the exact thing that
/// ships, with no TCP port and no browser (issue #45's own testing
/// decision).
///
/// Deliberately no CORS layer, in either environment (§0.15): production is
/// one origin behind Caddy, and development is one origin because Vite
/// proxies `/api`. A future contributor reaching for `tower_http::cors`
/// here would be solving a problem this deployment does not have.
///
/// Layers are added innermost-first: each `.layer()` call wraps everything
/// added so far, so the *last* one added ends up *outermost* — the first to
/// see a request and the last to see its response. The order here puts
/// [`CatchPanicLayer`] *inside* the security-headers and request-id layers
/// on purpose: a panic it catches turns into a 500 [`Response`], and that
/// response must still pass back out through both — carrying the security
/// headers, and logged inside the request-id span — rather than a panic
/// bypassing them the way it would if either sat inside the catch instead.
pub fn build_router(state: AppState) -> Router {
    finish_router(production_routes(), state)
}

fn production_routes() -> Router<AppState> {
    #[allow(unused_mut, reason = "reassigned only when test-support is enabled")]
    let mut router = Router::new()
        .route("/api/health", get(health))
        .route("/api/ready", get(ready))
        .route(
            "/api/session",
            get(session::who_am_i)
                .post(session::login)
                .delete(session::logout),
        )
        .route("/api/employers", get(employers::list_employers))
        .route(
            "/api/employers/{employer_id}/employments",
            get(employments::list_employments).post(employments::create_employment),
        )
        .route(
            "/api/employers/{employer_id}/employments/{employment_id}",
            get(employments::get_employment),
        );

    #[cfg(feature = "test-support")]
    {
        router = router
            .route("/__test/echo", axum::routing::post(test_support::echo))
            .route("/__test/panic", get(test_support::panic));
    }

    router
}

fn finish_router(router: Router<AppState>, state: AppState) -> Router {
    router
        .fallback(not_found)
        .layer(middleware::from_fn(require_salt_request_header))
        .layer(axum::extract::DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn(envelope_body_limit_error))
        .layer(CatchPanicLayer::custom(handle_panic))
        .layer(middleware::from_fn(security_headers))
        .layer(middleware::from_fn(request_id::middleware))
        .with_state(state)
}

/// The issue #47 probes are mounted only in this crate's unit-test binary.
/// Unlike a Cargo feature, `cfg(test)` cannot be selected for the deployed
/// library or server binary, which mechanically enforces the ticket's rule
/// that no authorization probe ships merely to be tested.
#[cfg(test)]
pub(crate) fn build_authorization_probe_router(state: AppState) -> Router {
    finish_router(
        production_routes()
            .route(
                "/__test/authorized-employer/{employer_id}",
                get(authorization_test_support::authorized_employer_probe),
            )
            .route(
                "/__test/authorized-employer/{employer_id}/owner-only",
                get(authorization_test_support::owner_only_probe),
            )
            .route(
                "/__test/authorized-employer/{employer_id}/resources/{resource_id}",
                get(authorization_test_support::authorized_employer_probe),
            ),
        state,
    )
}

/// Liveness: answers without touching the database (issue #45's own
/// acceptance criterion) — proof the process itself is up and serving.
async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

/// Readiness: a real database round trip, through `payroll-app`'s
/// [`payroll_app::ping`] — `salt-server` has no `sqlx` to write that query
/// itself (ADR-0018). A failure is mapped generically, never repeating
/// whatever PostgreSQL said: the exhaustive `PayrollAppError` mapping is
/// Spec 2's, not this one's, and a raw database error is exactly the "SQL
/// text in the body" issue #45 refuses.
async fn ready(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    payroll_app::ping(state.db())
        .await
        .map_err(ApiError::not_ready)?;
    Ok(Json(json!({ "status": "ok" })))
}

async fn not_found() -> ApiError {
    ApiError::not_found()
}

/// True for a request whose method is capable of changing state. Deliberately
/// a closed list rather than "not GET": `HEAD`, `OPTIONS`, `TRACE` and
/// `CONNECT` change nothing either, and refusing them for a missing header
/// would only break well-behaved clients issuing them.
fn is_mutating(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

/// The whole CSRF defence, together with the session cookie's future
/// `SameSite=Lax` (issue #45's Deep Instructions): a mutating request must
/// carry `X-Salt-Request: 1`. A cross-site form post cannot set a custom
/// header, so a browser-originated cross-site mutation never carries it.
async fn require_salt_request_header(request: Request, next: Next) -> Response {
    let carries_header = request
        .headers()
        .get("x-salt-request")
        .and_then(|value| value.to_str().ok())
        == Some("1");

    if is_mutating(request.method()) && !carries_header {
        return ApiError::salt_request_header_required().into_response();
    }

    next.run(request).await
}

/// Axum converts an over-limit body into a 413 extractor rejection before a
/// handler can return [`ApiError`]. Turn that framework response back into
/// Salt's documented envelope at the router boundary.
async fn envelope_body_limit_error(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    if response.status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError::payload_too_large().into_response()
    } else {
        response
    }
}

/// CSP, `X-Content-Type-Options` and frame-deny (issue #45's own acceptance
/// criteria). Caddy owns TLS and HSTS in front of this process — those are
/// not this layer's job (Deep Instructions).
async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; frame-ancestors 'none'"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    response
}

/// [`CatchPanicLayer`]'s handler: a panicking route must still answer the
/// documented envelope, not `tower_http`'s own plain-text default. There is
/// no request to read a body limit or header from here — only the panic
/// payload — which is exactly why the request id lives in a task-local
/// ([`request_id`]) rather than an extractor: this is the one call site that
/// cannot use an extractor at all.
fn handle_panic(payload: Box<dyn std::any::Any + Send + 'static>) -> Response {
    let message = panic_message(&payload);
    ApiError::internal(format!("handler panicked: {message}")).into_response()
}

fn panic_message(payload: &(dyn std::any::Any + Send + 'static)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// Routes that exist only so this crate's own `tests/router.rs` can drive
/// [`require_salt_request_header`], the body limit and [`CatchPanicLayer`]
/// against something real. Authorization probes are stricter: they live in
/// [`authorization_test_support`] under `cfg(test)`, not this selectable
/// feature, because issue #47 explicitly requires them to be mounted only in
/// a test binary.
#[cfg(feature = "test-support")]
mod test_support {
    use axum::Json;
    use axum::body::Bytes;
    use serde_json::{Value, json};

    pub async fn echo(body: Bytes) -> Json<Value> {
        Json(json!({ "bytes": body.len() }))
    }

    pub async fn panic() -> Json<Value> {
        panic!("deliberate panic from salt-server's own test-support route")
    }
}

#[cfg(test)]
mod authorization_test_support {
    use axum::Json;
    use payroll_app::MembershipRole;
    use serde_json::{Value, json};

    use crate::authorized_employer::AuthorizedEmployerContext;
    use crate::error::ApiError;

    /// Proves [`AuthorizedEmployerContext`] itself: any active member
    /// reaches this route and gets back exactly what the extractor resolved
    /// — nothing here re-derives or re-checks any of it.
    pub async fn authorized_employer_probe(context: AuthorizedEmployerContext) -> Json<Value> {
        Json(json!({
            "operatorId": context.operator_id().to_string(),
            "employerId": context.employer_id().to_string(),
            "role": role_str(context.role()),
            "actor": context.actor(),
        }))
    }

    /// Proves [`AuthorizedEmployerContext::require_role`]: reachable by any
    /// member, but only an `Owner` gets past the role check — the shape
    /// every future §0.6 Owner-only route is meant to follow.
    pub async fn owner_only_probe(
        context: AuthorizedEmployerContext,
    ) -> Result<Json<Value>, ApiError> {
        context.require_role(MembershipRole::Owner)?;
        Ok(Json(
            json!({ "operatorId": context.operator_id().to_string() }),
        ))
    }

    fn role_str(role: MembershipRole) -> &'static str {
        match role {
            MembershipRole::Owner => "owner",
            MembershipRole::PayrollOperator => "payrollOperator",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_methods_capable_of_mutating_state_need_the_header() {
        assert!(is_mutating(&Method::POST));
        assert!(is_mutating(&Method::PUT));
        assert!(is_mutating(&Method::PATCH));
        assert!(is_mutating(&Method::DELETE));
        assert!(!is_mutating(&Method::GET));
        assert!(!is_mutating(&Method::HEAD));
        assert!(!is_mutating(&Method::OPTIONS));
    }

    #[test]
    fn a_string_panic_payload_is_read_back() {
        let payload: Box<dyn std::any::Any + Send> = Box::new("boom".to_string());
        assert_eq!(panic_message(&*payload), "boom");
    }

    #[test]
    fn a_str_panic_payload_is_read_back() {
        let payload: Box<dyn std::any::Any + Send> = Box::new("boom");
        assert_eq!(panic_message(&*payload), "boom");
    }

    #[test]
    fn an_unrecognised_panic_payload_is_named_rather_than_panicking_itself() {
        let payload: Box<dyn std::any::Any + Send> = Box::new(42_i32);
        assert_eq!(panic_message(&*payload), "non-string panic payload");
    }
}
