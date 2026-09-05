//! Proves issue #46's own acceptance criteria over HTTP: `POST`, `DELETE`
//! and `GET /api/session`, driven with `tower::ServiceExt::oneshot` against
//! the real router — no TCP port, no browser (issue #45's own testing
//! decision, which this crate inherits).

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use payroll_app::{DatabaseConfig, MembershipRole, OperatorId, SaltDatabase};
use salt_server::{AppState, build_router};
use serde_json::{Value, json};
use tower::ServiceExt;

/// Mirrors `tests/router.rs`'s own `test_database_config`: this crate has no
/// `sqlx` (ADR-0018), so these tests connect to whatever `DATABASE_URL`
/// already names, already migrated.
fn test_database_config() -> DatabaseConfig {
    let url = std::env::var("DATABASE_URL").expect(
        "DATABASE_URL must name an already-migrated database for salt-server's own tests \
         (see crates/salt-server/tests/session.rs)",
    );
    DatabaseConfig {
        url,
        max_connections: 2,
        acquire_timeout: Duration::from_secs(10),
        idle_timeout: None,
    }
}

async fn test_db() -> SaltDatabase {
    SaltDatabase::connect(&test_database_config())
        .await
        .expect("connect to the local, migrated test database (see AGENTS.md)")
}

/// Every test needs its own database handle (used directly to seed an
/// Operator) and its own router built over it — `SaltDatabase` has no
/// `Clone`, so the router is rebuilt per call from a fresh connection
/// against the same already-migrated database rather than shared.
async fn router() -> Router {
    build_router(AppState::new(test_db().await, true))
}

/// Unlike `payroll-app`'s own `#[sqlx::test]` suite, every test here shares
/// one persistent, already-migrated database (`AGENTS.md`) rather than a
/// disposable one of its own — so an Operator's email, unique across the
/// whole table, must be unique across the whole suite too. A fresh
/// [`uuid::Uuid`] per call is what makes that true without any test having
/// to coordinate with another.
fn unique_email(local_part: &str) -> String {
    format!("{local_part}-{}@example.com", uuid::Uuid::new_v4())
}

async fn create_operator(email: &str, password: &str) -> OperatorId {
    payroll_app::create_operator(&test_db().await, email, "Test Operator", password)
        .await
        .unwrap()
}

fn login_request(email: &str, password: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/api/session")
        .header("x-salt-request", "1")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "email": email, "password": password }).to_string(),
        ))
        .unwrap()
}

fn get_session_request(cookie: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("GET").uri("/api/session");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    builder.body(Body::empty()).unwrap()
}

fn delete_session_request(cookie: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("DELETE")
        .uri("/api/session")
        .header("x-salt-request", "1");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    builder.body(Body::empty()).unwrap()
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Extracts the `salt_session=...` pair out of a `Set-Cookie` header, so a
/// later request can carry it back in its own `Cookie` header the way a
/// browser would.
fn session_cookie_pair(response: &axum::response::Response) -> String {
    let set_cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .expect("a login or logout response always carries Set-Cookie")
        .to_str()
        .unwrap();
    set_cookie
        .split(';')
        .next()
        .expect("Set-Cookie always carries at least the name=value pair")
        .to_string()
}

#[tokio::test]
async fn logging_in_with_the_right_credentials_sets_a_cookie_with_every_documented_attribute() {
    let email = unique_email("alice");
    create_operator(&email, "correct horse battery staple").await;

    let response = router()
        .await
        .oneshot(login_request(&email, "correct horse battery staple"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let set_cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .expect("a successful login must set the session cookie")
        .to_str()
        .unwrap();
    assert!(set_cookie.starts_with("salt_session="), "{set_cookie}");
    assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
    assert!(set_cookie.contains("SameSite=Lax"), "{set_cookie}");
    assert!(set_cookie.contains("Path=/"), "{set_cookie}");
    assert!(set_cookie.contains("Secure"), "{set_cookie}");
    assert!(!set_cookie.contains("Domain"), "{set_cookie}");
}

#[tokio::test]
async fn a_wrong_password_an_unknown_email_and_a_disabled_operator_all_answer_the_same_401() {
    let disabled_email = unique_email("bob");
    let disabled = create_operator(&disabled_email, "correct horse battery staple").await;
    payroll_app::disable_operator(&test_db().await, &disabled)
        .await
        .unwrap();
    let active_email = unique_email("alice");
    create_operator(&active_email, "correct horse battery staple").await;
    let unknown_email = unique_email("nobody");

    for request in [
        login_request(&active_email, "wrong password"),
        login_request(&unknown_email, "anything at all"),
        login_request(&disabled_email, "correct horse battery staple"),
    ] {
        let response = router().await.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(
            response.headers().get(header::SET_COOKIE).is_none(),
            "a failed login is never handed a session cookie"
        );
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "invalid_credentials");
        assert!(json["error"]["details"].is_null());
    }
}

#[tokio::test]
async fn a_locked_account_answers_the_same_401_as_every_other_failure() {
    // Ten failures inside the window lock the account (issue #42), after
    // which even the right password is refused — and refused identically,
    // which is the criterion this test exists for. Driven through the real
    // route rather than the use case, so the mapping is what is proven.
    let email = unique_email("alice");
    create_operator(&email, "correct horse battery staple").await;
    let router = router().await;

    for _ in 0..10 {
        let response = router
            .clone()
            .oneshot(login_request(&email, "wrong password"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    let response = router
        .oneshot(login_request(&email, "correct horse battery staple"))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        response.headers().get(header::SET_COOKIE).is_none(),
        "a locked account is never handed a session cookie"
    );
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "invalid_credentials");
    assert!(json["error"]["details"].is_null());
}

#[tokio::test]
async fn an_operator_disabled_mid_session_is_refused_on_the_next_request() {
    let email = unique_email("alice");
    let operator_id = create_operator(&email, "correct horse battery staple").await;
    let login_response = router()
        .await
        .oneshot(login_request(&email, "correct horse battery staple"))
        .await
        .unwrap();
    let cookie = session_cookie_pair(&login_response);
    assert_eq!(
        router()
            .await
            .oneshot(get_session_request(Some(&cookie)))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    payroll_app::disable_operator(&test_db().await, &operator_id)
        .await
        .unwrap();

    let response = router()
        .await
        .oneshot(get_session_request(Some(&cookie)))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "unauthenticated");
}

#[tokio::test]
async fn login_without_the_salt_request_header_is_refused() {
    let email = unique_email("alice");
    create_operator(&email, "correct horse battery staple").await;

    let request = Request::builder()
        .method("POST")
        .uri("/api/session")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "email": email, "password": "correct horse battery staple" }).to_string(),
        ))
        .unwrap();

    let response = router().await.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "salt_request_header_required");
}

#[tokio::test]
async fn a_malformed_login_body_answers_400() {
    let request = Request::builder()
        .method("POST")
        .uri("/api/session")
        .header("x-salt-request", "1")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("not json"))
        .unwrap();

    let response = router().await.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "malformed_request");
}

#[tokio::test]
async fn who_am_i_returns_the_operator_and_their_active_memberships() {
    let db = test_db().await;
    let email = unique_email("alice");
    let operator_id = create_operator(&email, "correct horse battery staple").await;
    let employer_id = payroll_app::create_employer(
        &db,
        "Acme Corp",
        payroll::PaySchedule::new(payroll::PeriodEndDay::LastDayOfMonth),
        "test-setup",
    )
    .await
    .unwrap();
    payroll_app::create_employer_membership(&db, &operator_id, &employer_id, MembershipRole::Owner)
        .await
        .unwrap();

    let login_response = router()
        .await
        .oneshot(login_request(&email, "correct horse battery staple"))
        .await
        .unwrap();
    let cookie = session_cookie_pair(&login_response);

    let response = router()
        .await
        .oneshot(get_session_request(Some(&cookie)))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["operator"]["email"], email);
    assert_eq!(json["operator"]["displayName"], "Test Operator");
    assert_eq!(json["memberships"].as_array().unwrap().len(), 1);
    assert_eq!(json["memberships"][0]["employerId"], employer_id.as_str());
    assert_eq!(json["memberships"][0]["role"], "owner");
    assert_eq!(json["memberships"][0]["name"], "Acme Corp");
}

#[tokio::test]
async fn a_revoked_membership_does_not_appear_in_who_am_i() {
    let db = test_db().await;
    let email = unique_email("alice");
    let operator_id = create_operator(&email, "correct horse battery staple").await;
    let employer_id = payroll_app::create_employer(
        &db,
        "Acme Corp",
        payroll::PaySchedule::new(payroll::PeriodEndDay::LastDayOfMonth),
        "test-setup",
    )
    .await
    .unwrap();
    payroll_app::create_employer_membership(&db, &operator_id, &employer_id, MembershipRole::Owner)
        .await
        .unwrap();
    payroll_app::revoke_employer_membership(&db, &operator_id, &employer_id)
        .await
        .unwrap();

    let login_response = router()
        .await
        .oneshot(login_request(&email, "correct horse battery staple"))
        .await
        .unwrap();
    let cookie = session_cookie_pair(&login_response);

    let response = router()
        .await
        .oneshot(get_session_request(Some(&cookie)))
        .await
        .unwrap();

    let json = body_json(response).await;
    assert_eq!(json["memberships"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn who_am_i_without_a_cookie_answers_401() {
    let response = router()
        .await
        .oneshot(get_session_request(None))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "unauthenticated");
}

#[tokio::test]
async fn who_am_i_with_a_forged_cookie_answers_401() {
    let response = router()
        .await
        .oneshot(get_session_request(Some("salt_session=not-a-real-token")))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn logout_deletes_the_session_and_the_same_cookie_then_answers_401() {
    let email = unique_email("alice");
    create_operator(&email, "correct horse battery staple").await;

    let login_response = router()
        .await
        .oneshot(login_request(&email, "correct horse battery staple"))
        .await
        .unwrap();
    let cookie = session_cookie_pair(&login_response);

    let logout_response = router()
        .await
        .oneshot(delete_session_request(Some(&cookie)))
        .await
        .unwrap();
    assert_eq!(logout_response.status(), StatusCode::OK);
    let cleared_cookie = logout_response
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(cleared_cookie.contains("Max-Age=0"), "{cleared_cookie}");

    let who_am_i_response = router()
        .await
        .oneshot(get_session_request(Some(&cookie)))
        .await
        .unwrap();
    assert_eq!(who_am_i_response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn logout_without_a_salt_request_header_is_refused() {
    let email = unique_email("alice");
    create_operator(&email, "correct horse battery staple").await;
    let login_response = router()
        .await
        .oneshot(login_request(&email, "correct horse battery staple"))
        .await
        .unwrap();
    let cookie = session_cookie_pair(&login_response);

    let request = Request::builder()
        .method("DELETE")
        .uri("/api/session")
        .header(header::COOKIE, &cookie)
        .body(Body::empty())
        .unwrap();

    let response = router().await.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn logout_with_no_cookie_is_not_refused() {
    let response = router()
        .await
        .oneshot(delete_session_request(None))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn logging_in_twice_allows_both_sessions_concurrently() {
    let email = unique_email("alice");
    create_operator(&email, "correct horse battery staple").await;

    let first_login = router()
        .await
        .oneshot(login_request(&email, "correct horse battery staple"))
        .await
        .unwrap();
    let first_cookie = session_cookie_pair(&first_login);

    let second_login = router()
        .await
        .oneshot(login_request(&email, "correct horse battery staple"))
        .await
        .unwrap();
    let second_cookie = session_cookie_pair(&second_login);

    assert_ne!(first_cookie, second_cookie);

    for cookie in [&first_cookie, &second_cookie] {
        let response = router()
            .await
            .oneshot(get_session_request(Some(cookie)))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
