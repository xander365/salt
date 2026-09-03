//! Proves `GET /api/employers` (issue #47, §0.22): the authenticated
//! Operator's own Employers, named and roled. Driven with
//! `tower::ServiceExt::oneshot` against the real router, the same discipline
//! `tests/session.rs` already follows.

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use payroll_app::{DatabaseConfig, MembershipRole, OperatorId, SaltDatabase};
use salt_server::{AppState, build_router};
use tower::ServiceExt;

fn test_database_config() -> DatabaseConfig {
    let url = std::env::var("DATABASE_URL").expect(
        "DATABASE_URL must name an already-migrated database for salt-server's own tests \
         (see crates/salt-server/tests/employers.rs)",
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

fn get_employers_request(cookie: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("GET").uri("/api/employers");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    builder.body(Body::empty()).unwrap()
}

async fn body_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn without_a_session_the_route_answers_401() {
    let response = router()
        .await
        .oneshot(get_employers_request(None))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn it_lists_only_the_operators_active_employers_named_and_roled() {
    let db = test_db().await;
    let email = unique_email("alice");
    let operator_id = create_operator(&email).await;

    let own_employer = payroll_app::create_employer(
        &db,
        "Acme Corp",
        payroll::PaySchedule::new(payroll::PeriodEndDay::LastDayOfMonth),
        "test-setup",
    )
    .await
    .unwrap();
    payroll_app::create_employer_membership(
        &db,
        &operator_id,
        &own_employer,
        MembershipRole::Owner,
    )
    .await
    .unwrap();

    let second_own_employer = payroll_app::create_employer(
        &db,
        "Beta Corp",
        payroll::PaySchedule::new(payroll::PeriodEndDay::LastDayOfMonth),
        "test-setup",
    )
    .await
    .unwrap();
    payroll_app::create_employer_membership(
        &db,
        &operator_id,
        &second_own_employer,
        MembershipRole::PayrollOperator,
    )
    .await
    .unwrap();

    let revoked_employer = payroll_app::create_employer(
        &db,
        "Revoked Co",
        payroll::PaySchedule::new(payroll::PeriodEndDay::LastDayOfMonth),
        "test-setup",
    )
    .await
    .unwrap();
    payroll_app::create_employer_membership(
        &db,
        &operator_id,
        &revoked_employer,
        MembershipRole::PayrollOperator,
    )
    .await
    .unwrap();
    payroll_app::revoke_employer_membership(&db, &operator_id, &revoked_employer)
        .await
        .unwrap();

    let other_employer = payroll_app::create_employer(
        &db,
        "Someone Else's Co",
        payroll::PaySchedule::new(payroll::PeriodEndDay::LastDayOfMonth),
        "test-setup",
    )
    .await
    .unwrap();
    let other_operator_id = create_operator(&unique_email("bob")).await;
    payroll_app::create_employer_membership(
        &db,
        &other_operator_id,
        &other_employer,
        MembershipRole::Owner,
    )
    .await
    .unwrap();

    let cookie = login(&email).await;

    let response = router()
        .await
        .oneshot(get_employers_request(Some(&cookie)))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let employers = json["employers"].as_array().unwrap();
    assert_eq!(employers.len(), 2);

    let first = employers
        .iter()
        .find(|employer| employer["employerId"] == own_employer.to_string())
        .expect("the Owner membership is returned");
    assert_eq!(first["name"], "Acme Corp");
    assert_eq!(first["role"], "owner");

    let second = employers
        .iter()
        .find(|employer| employer["employerId"] == second_own_employer.to_string())
        .expect("the PayrollOperator membership is returned");
    assert_eq!(second["name"], "Beta Corp");
    assert_eq!(second["role"], "payrollOperator");
}

#[tokio::test]
async fn a_disabled_operator_is_refused_401() {
    let db = test_db().await;
    let email = unique_email("alice");
    let operator_id = create_operator(&email).await;
    let cookie = login(&email).await;

    payroll_app::disable_operator(&db, &operator_id)
        .await
        .unwrap();

    let response = router()
        .await
        .oneshot(get_employers_request(Some(&cookie)))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
