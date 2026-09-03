//! Proves `AuthorizedEmployerContext` (issue #47) over HTTP, through the
//! `test-support`-only probe routes `router.rs` ships for exactly this
//! purpose (its own Deep Instructions: nothing production-facing exists yet
//! to exercise the extractor otherwise). Driven with
//! `tower::ServiceExt::oneshot` against the real router, the same discipline
//! `tests/session.rs` and `tests/router.rs` already follow.

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use payroll_app::{DatabaseConfig, MembershipRole, OperatorId, SaltDatabase};
use salt_server::{AppState, build_router};
use serde_json::Value;
use tower::ServiceExt;

fn test_database_config() -> DatabaseConfig {
    let url = std::env::var("DATABASE_URL").expect(
        "DATABASE_URL must name an already-migrated database for salt-server's own tests \
         (see crates/salt-server/tests/authorized_employer.rs)",
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

async fn router() -> Router {
    build_router(AppState::new(test_db().await, true))
}

fn unique_email(local_part: &str) -> String {
    format!("{local_part}-{}@example.com", uuid::Uuid::new_v4())
}

async fn create_operator(email: &str) -> OperatorId {
    payroll_app::create_operator(
        &test_db().await,
        email,
        "Test Operator",
        "correct horse battery staple",
    )
    .await
    .unwrap()
}

async fn create_employer() -> String {
    payroll_app::create_employer(
        &test_db().await,
        "Acme Corp",
        payroll::PaySchedule::new(payroll::PeriodEndDay::LastDayOfMonth),
        "test-setup",
    )
    .await
    .unwrap()
    .to_string()
}

async fn login(email: &str) -> String {
    let response = router()
        .await
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/session")
                .header("x-salt-request", "1")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "email": email,
                        "password": "correct horse battery staple",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    let set_cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .expect("a successful login always sets the session cookie")
        .to_str()
        .unwrap();
    set_cookie
        .split(';')
        .next()
        .expect("Set-Cookie always carries at least the name=value pair")
        .to_string()
}

fn probe_request(employer_id: &str, cookie: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("GET")
        .uri(format!("/__test/authorized-employer/{employer_id}"));
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    builder.body(Body::empty()).unwrap()
}

fn owner_only_probe_request(employer_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!(
            "/__test/authorized-employer/{employer_id}/owner-only"
        ))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn an_unauthenticated_request_answers_401() {
    let employer_id = create_employer().await;

    let response = router()
        .await
        .oneshot(probe_request(&employer_id, None))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "unauthenticated");
}

#[tokio::test]
async fn a_forged_session_cookie_answers_401() {
    let employer_id = create_employer().await;

    let response = router()
        .await
        .oneshot(probe_request(
            &employer_id,
            Some("salt_session=not-a-real-token"),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_member_reaches_their_own_employer_and_the_context_carries_the_right_facts() {
    let db = test_db().await;
    let email = unique_email("alice");
    let operator_id = create_operator(&email).await;
    let employer_id = create_employer().await;
    payroll_app::create_employer_membership(
        &db,
        &operator_id,
        &payroll_app::EmployerId::new(employer_id.clone()),
        MembershipRole::PayrollOperator,
    )
    .await
    .unwrap();
    let cookie = login(&email).await;

    let response = router()
        .await
        .oneshot(probe_request(&employer_id, Some(&cookie)))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["operatorId"], operator_id.as_str());
    assert_eq!(json["employerId"], employer_id);
    assert_eq!(json["role"], "payrollOperator");
    assert_eq!(json["actor"], format!("operator:{operator_id}"));
}

#[tokio::test]
async fn a_non_member_gets_404_for_an_employer_that_does_exist() {
    let email = unique_email("alice");
    create_operator(&email).await;
    let employer_id = create_employer().await;
    let cookie = login(&email).await;

    let response = router()
        .await
        .oneshot(probe_request(&employer_id, Some(&cookie)))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "not_found");
}

#[tokio::test]
async fn an_unknown_employer_id_also_answers_404_not_401() {
    let email = unique_email("alice");
    create_operator(&email).await;
    let cookie = login(&email).await;

    let response = router()
        .await
        .oneshot(probe_request("no-such-employer", Some(&cookie)))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_payroll_operator_is_refused_403_on_a_route_that_demands_owner() {
    let db = test_db().await;
    let email = unique_email("alice");
    let operator_id = create_operator(&email).await;
    let employer_id = create_employer().await;
    payroll_app::create_employer_membership(
        &db,
        &operator_id,
        &payroll_app::EmployerId::new(employer_id.clone()),
        MembershipRole::PayrollOperator,
    )
    .await
    .unwrap();
    let cookie = login(&email).await;

    let response = router()
        .await
        .oneshot(owner_only_probe_request(&employer_id, &cookie))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "forbidden");
}

#[tokio::test]
async fn an_owner_is_let_through_a_route_that_demands_owner() {
    let db = test_db().await;
    let email = unique_email("alice");
    let operator_id = create_operator(&email).await;
    let employer_id = create_employer().await;
    payroll_app::create_employer_membership(
        &db,
        &operator_id,
        &payroll_app::EmployerId::new(employer_id.clone()),
        MembershipRole::Owner,
    )
    .await
    .unwrap();
    let cookie = login(&email).await;

    let response = router()
        .await
        .oneshot(owner_only_probe_request(&employer_id, &cookie))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_membership_revoked_mid_session_is_refused_on_the_next_request() {
    let db = test_db().await;
    let email = unique_email("alice");
    let operator_id = create_operator(&email).await;
    let employer_id = create_employer().await;
    let employer_id_typed = payroll_app::EmployerId::new(employer_id.clone());
    payroll_app::create_employer_membership(
        &db,
        &operator_id,
        &employer_id_typed,
        MembershipRole::Owner,
    )
    .await
    .unwrap();
    let cookie = login(&email).await;

    assert_eq!(
        router()
            .await
            .oneshot(probe_request(&employer_id, Some(&cookie)))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    payroll_app::revoke_employer_membership(&db, &operator_id, &employer_id_typed)
        .await
        .unwrap();

    let response = router()
        .await
        .oneshot(probe_request(&employer_id, Some(&cookie)))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_operator_disabled_mid_session_is_refused_401_on_the_next_request() {
    let db = test_db().await;
    let email = unique_email("alice");
    let operator_id = create_operator(&email).await;
    let employer_id = create_employer().await;
    payroll_app::create_employer_membership(
        &db,
        &operator_id,
        &payroll_app::EmployerId::new(employer_id.clone()),
        MembershipRole::Owner,
    )
    .await
    .unwrap();
    let cookie = login(&email).await;

    assert_eq!(
        router()
            .await
            .oneshot(probe_request(&employer_id, Some(&cookie)))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    payroll_app::disable_operator(&db, &operator_id)
        .await
        .unwrap();

    let response = router()
        .await
        .oneshot(probe_request(&employer_id, Some(&cookie)))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "unauthenticated");
}
