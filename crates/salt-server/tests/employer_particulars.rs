//! Proves `GET`/`PUT /api/employers/{e}/particulars` (issue #71, parent #70
//! D-7): the first Owner-only route. Same `oneshot`-against-the-real-router
//! discipline as `tests/employment_facts.rs`.

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use payroll_app::{DatabaseConfig, MembershipRole, OperatorId, SaltDatabase};
use salt_server::{AppState, build_router};
use serde_json::{Value, json};
use tower::ServiceExt;

fn test_database_config() -> DatabaseConfig {
    let url = std::env::var("DATABASE_URL").expect(
        "DATABASE_URL must name an already-migrated database for salt-server's own tests \
         (see crates/salt-server/tests/employer_particulars.rs)",
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

async fn a_fresh_employer() -> String {
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

/// An Operator, logged in, holding `role` on a fresh Employer of their own.
/// Returns the cookie and the Employer's own id.
async fn an_operator_with_role(role: MembershipRole) -> (String, String) {
    let db = test_db().await;
    let email = unique_email("alice");
    let operator_id = create_operator(&email).await;
    let employer_id = a_fresh_employer().await;
    payroll_app::create_employer_membership(
        &db,
        &operator_id,
        &payroll::EmployerId::new(employer_id.clone()),
        role,
    )
    .await
    .unwrap();
    let cookie = login(&email).await;
    (cookie, employer_id)
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn get_request(employer_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!("/api/employers/{employer_id}/particulars"))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn put_request(employer_id: &str, cookie: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(format!("/api/employers/{employer_id}/particulars"))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-salt-request", "1")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn particulars_body() -> Value {
    json!({
        "registeredName": "Acme Corp (Pty) Ltd",
        "addressLine1": "1 Independence Ave",
        "city": "Windhoek",
    })
}

#[tokio::test]
async fn an_owner_can_record_particulars_and_read_them_back() {
    let (cookie, employer_id) = an_operator_with_role(MembershipRole::Owner).await;

    let put_response = router()
        .await
        .oneshot(put_request(&employer_id, &cookie, particulars_body()))
        .await
        .unwrap();
    assert_eq!(put_response.status(), StatusCode::OK);

    // Survives a reload: a fresh request, against a fresh router, sees the
    // same row.
    let get_response = router()
        .await
        .oneshot(get_request(&employer_id, &cookie))
        .await
        .unwrap();
    assert_eq!(get_response.status(), StatusCode::OK);
    let json = body_json(get_response).await;
    assert_eq!(json["registeredName"], "Acme Corp (Pty) Ltd");
    assert_eq!(json["addressLine1"], "1 Independence Ave");
    assert_eq!(json["city"], "Windhoek");
    assert!(
        json["createdBy"].as_str().unwrap().starts_with("operator:"),
        "the actor is always taken from the authorized context, never the request body: {json}"
    );
}

#[tokio::test]
async fn unrecorded_particulars_read_back_as_null_not_404() {
    let (cookie, employer_id) = an_operator_with_role(MembershipRole::Owner).await;

    let response = router()
        .await
        .oneshot(get_request(&employer_id, &cookie))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await, Value::Null);
}

#[tokio::test]
async fn a_payroll_operator_reads_200_but_writes_403() {
    let (cookie, employer_id) = an_operator_with_role(MembershipRole::PayrollOperator).await;

    let get_response = router()
        .await
        .oneshot(get_request(&employer_id, &cookie))
        .await
        .unwrap();
    assert_eq!(get_response.status(), StatusCode::OK);

    let put_response = router()
        .await
        .oneshot(put_request(&employer_id, &cookie, particulars_body()))
        .await
        .unwrap();
    assert_eq!(put_response.status(), StatusCode::FORBIDDEN);
    let json = body_json(put_response).await;
    assert_eq!(json["error"]["code"], "forbidden");

    // The 403 wrote nothing.
    let get_after = router()
        .await
        .oneshot(get_request(&employer_id, &cookie))
        .await
        .unwrap();
    assert_eq!(body_json(get_after).await, Value::Null);
}

#[tokio::test]
async fn a_blank_registered_name_is_refused() {
    let (cookie, employer_id) = an_operator_with_role(MembershipRole::Owner).await;

    let response = router()
        .await
        .oneshot(put_request(
            &employer_id,
            &cookie,
            json!({ "registeredName": "   ", "addressLine1": "1 Main St", "city": "Windhoek" }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "employer_particulars_registered_name_cannot_be_empty"
    );
}

#[tokio::test]
async fn an_employer_id_belonging_to_another_employer_is_refused_like_one_that_does_not_exist() {
    let (cookie, _own_employer_id) = an_operator_with_role(MembershipRole::Owner).await;
    let other_employer_id = a_fresh_employer().await;

    let get_response = router()
        .await
        .oneshot(get_request(&other_employer_id, &cookie))
        .await
        .unwrap();
    assert_eq!(get_response.status(), StatusCode::NOT_FOUND);

    let unknown_response = router()
        .await
        .oneshot(get_request("no-such-employer", &cookie))
        .await
        .unwrap();
    assert_eq!(unknown_response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(get_response).await["error"]["code"],
        body_json(unknown_response).await["error"]["code"],
    );
}

#[tokio::test]
async fn correcting_an_existing_row_demands_a_reason_and_logs_it() {
    let (cookie, employer_id) = an_operator_with_role(MembershipRole::Owner).await;
    let first = router()
        .await
        .oneshot(put_request(&employer_id, &cookie, particulars_body()))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let mut correction = particulars_body();
    correction["registeredName"] = json!("Acme Holdings (Pty) Ltd");

    let unreasoned = router()
        .await
        .oneshot(put_request(&employer_id, &cookie, correction.clone()))
        .await
        .unwrap();
    assert_eq!(unreasoned.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(unreasoned).await["error"]["code"],
        "employer_particulars_correction_reason_cannot_be_empty"
    );

    correction["reason"] = json!("registered a new legal name");
    let reasoned = router()
        .await
        .oneshot(put_request(&employer_id, &cookie, correction))
        .await
        .unwrap();
    assert_eq!(reasoned.status(), StatusCode::OK);

    let get_response = router()
        .await
        .oneshot(get_request(&employer_id, &cookie))
        .await
        .unwrap();
    assert_eq!(
        body_json(get_response).await["registeredName"],
        "Acme Holdings (Pty) Ltd"
    );
}
