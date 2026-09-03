//! The router seam issue #45's own Testing Decisions names: `GET
//! /api/health`, `GET /api/ready`, the error envelope's 404 fallback, the
//! `X-Salt-Request` mutation guard and the JSON body limit — driven with
//! `tower::ServiceExt::oneshot` against the real router the library builds,
//! with no TCP port and no browser.
//!
//! `/__test/echo` is the probe route `test-support` (this crate's own
//! `dev-dependencies`, see `Cargo.toml`) compiles in: the only route in this
//! spec that reads a request body, needed to exercise the header guard and
//! the body limit against something real rather than asserting on the
//! middleware's source directly.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use payroll_app::{DatabaseConfig, SaltDatabase};
use salt_server::{AppState, build_router};
use tower::ServiceExt;

const MAX_BODY_BYTES: usize = 256 * 1024;

/// A `DATABASE_URL` naming an already-migrated, persistent database.
///
/// Unlike `payroll-app`'s own `#[sqlx::test]` suite, this crate has no
/// `sqlx` to spin up a fresh, disposable database per test (ADR-0018): these
/// tests connect to whatever `DATABASE_URL` already names, so it must
/// already be migrated to `payroll-app`'s current schema (`sqlx migrate run
/// --source crates/payroll-app/migrations --database-url ...`) before this
/// suite runs.
fn test_database_config() -> DatabaseConfig {
    let url = std::env::var("DATABASE_URL").expect(
        "DATABASE_URL must name an already-migrated database for salt-server's own tests \
         (see crates/salt-server/tests/router.rs)",
    );
    DatabaseConfig {
        url,
        max_connections: 2,
        acquire_timeout: Duration::from_secs(10),
        idle_timeout: None,
    }
}

async fn test_router() -> axum::Router {
    let db = SaltDatabase::connect(&test_database_config())
        .await
        .expect("connect to the local, migrated test database (see AGENTS.md)");
    build_router(AppState::new(db, true))
}

fn get(path: &str) -> Request<Body> {
    Request::builder().uri(path).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn health_answers_ok_without_a_database() {
    let response = test_router()
        .await
        .oneshot(get("/api/health"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn ready_performs_a_real_database_round_trip_and_answers_ok() {
    let response = test_router()
        .await
        .oneshot(get("/api/ready"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn an_unmatched_route_answers_404_with_the_error_envelope() {
    let response = test_router()
        .await
        .oneshot(get("/api/does-not-exist"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "not_found");
    assert!(json["error"]["details"].is_null());
}

#[tokio::test]
async fn every_response_carries_the_security_headers() {
    let response = test_router()
        .await
        .oneshot(get("/api/health"))
        .await
        .unwrap();

    let headers = response.headers();
    assert!(headers.contains_key(header::CONTENT_SECURITY_POLICY));
    assert_eq!(
        headers.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
        "nosniff"
    );
    assert_eq!(headers.get(header::X_FRAME_OPTIONS).unwrap(), "DENY");
}

/// Even the 404 fallback must carry them: the layer wraps the whole router,
/// `fallback` included.
#[tokio::test]
async fn the_404_fallback_also_carries_the_security_headers() {
    let response = test_router()
        .await
        .oneshot(get("/api/does-not-exist"))
        .await
        .unwrap();

    assert!(
        response
            .headers()
            .contains_key(header::CONTENT_SECURITY_POLICY)
    );
}

#[tokio::test]
async fn a_mutating_request_without_the_header_is_refused_with_400() {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/__test/echo")
        .body(Body::from("{}"))
        .unwrap();

    let response = test_router().await.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "salt_request_header_required");
}

#[tokio::test]
async fn a_mutating_request_with_the_header_is_let_through() {
    let request = Request::builder()
        .method(Method::POST)
        .uri("/__test/echo")
        .header("x-salt-request", "1")
        .body(Body::from("{}"))
        .unwrap();

    let response = test_router().await.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

/// A `GET` needs no header at all — only a method capable of mutating state
/// does.
#[tokio::test]
async fn a_get_request_needs_no_salt_request_header() {
    let response = test_router()
        .await
        .oneshot(get("/api/health"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_body_over_the_256kb_limit_is_refused() {
    let oversized = "x".repeat(MAX_BODY_BYTES + 1);
    let request = Request::builder()
        .method(Method::POST)
        .uri("/__test/echo")
        .header("x-salt-request", "1")
        .body(Body::from(oversized))
        .unwrap();

    let response = test_router().await.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "payload_too_large");
    assert!(json["error"]["details"].is_null());
}

/// A handler panic must not crash the server or bypass the transport
/// layers: it comes back as the documented 500 envelope, still carrying the
/// security headers and a request id — proof that `CatchPanicLayer` sits
/// where `crate::router::build_router`'s own layer-ordering comment says it
/// must (inside the security-headers and request-id layers, not outside
/// them).
#[tokio::test]
async fn a_handler_panic_is_caught_and_answered_with_the_envelope() {
    let response = test_router()
        .await
        .oneshot(get("/__test/panic"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        response
            .headers()
            .contains_key(header::CONTENT_SECURITY_POLICY)
    );

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], "internal_error");
    assert!(json["error"]["details"]["requestId"].is_string());
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(
        !text.contains("deliberate panic"),
        "the panic message must not reach the response body, got {text}"
    );
}

#[tokio::test]
async fn a_body_at_the_256kb_limit_is_accepted() {
    let at_limit = "x".repeat(MAX_BODY_BYTES);
    let request = Request::builder()
        .method(Method::POST)
        .uri("/__test/echo")
        .header("x-salt-request", "1")
        .body(Body::from(at_limit))
        .unwrap();

    let response = test_router().await.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

/// The header guard is an exact match on `1`, not a presence check: a header
/// that is there but says something else must be refused exactly as an
/// absent one is, so a client cannot opt out of the CSRF defence by sending
/// the header set to anything at all.
#[tokio::test]
async fn a_salt_request_header_with_the_wrong_value_is_refused() {
    for value in ["0", "true", "", "1 "] {
        let request = Request::builder()
            .method(Method::POST)
            .uri("/__test/echo")
            .header("x-salt-request", value)
            .body(Body::from("{}"))
            .unwrap();

        let response = test_router().await.oneshot(request).await.unwrap();

        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "X-Salt-Request: {value:?} must be refused"
        );
    }
}

/// Two requests must not share a request id, or the id in `details.requestId`
/// cannot pick one request's log lines out of a running server's output —
/// which is the only reason issue #45 asks for it.
#[tokio::test]
async fn each_request_gets_its_own_request_id() {
    async fn request_id_of_a_500() -> String {
        let response = test_router()
            .await
            .oneshot(get("/__test/panic"))
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        json["error"]["details"]["requestId"]
            .as_str()
            .expect("a 500 carries details.requestId")
            .to_string()
    }

    let first = request_id_of_a_500().await;
    let second = request_id_of_a_500().await;

    assert_ne!(first, second);
}

/// A `HEAD` is not a mutation, and Axum answers it from the same `GET`
/// handler — so it must pass the header guard untouched rather than being
/// refused for lacking a header only mutations need.
#[tokio::test]
async fn a_head_request_reaches_the_health_route() {
    let request = Request::builder()
        .method(Method::HEAD)
        .uri("/api/health")
        .body(Body::empty())
        .unwrap();

    let response = test_router().await.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}
