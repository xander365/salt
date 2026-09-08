//! Proves `GET`/`PUT /api/employers/{e}/people/{p}/particulars` and `PUT
//! /api/employers/{e}/people/{p}/name` (issue #72, parent #70 D-7). Same
//! `oneshot`-against-the-real-router discipline as `tests/employer_particulars.rs`.

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
         (see crates/salt-server/tests/person_particulars.rs)",
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

/// Creates a Person (and its first Employment) by full name, and returns the
/// Person's own id.
async fn a_person(employer_id: &str, cookie: &str, full_name: &str) -> String {
    let response = router()
        .await
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/employers/{employer_id}/employments"))
                .header(header::COOKIE, cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-salt-request", "1")
                .body(Body::from(
                    json!({ "fullName": full_name, "startDate": "2026-03-01" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await["personId"]
        .as_str()
        .unwrap()
        .to_string()
}

fn get_request(employer_id: &str, person_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!(
            "/api/employers/{employer_id}/people/{person_id}/particulars"
        ))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn put_particulars_request(
    employer_id: &str,
    person_id: &str,
    cookie: &str,
    body: Value,
) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(format!(
            "/api/employers/{employer_id}/people/{person_id}/particulars"
        ))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-salt-request", "1")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn put_name_request(
    employer_id: &str,
    person_id: &str,
    cookie: &str,
    body: Value,
) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(format!(
            "/api/employers/{employer_id}/people/{person_id}/name"
        ))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-salt-request", "1")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn particulars_body() -> Value {
    json!({
        "identityNumber": "800101 5000 1",
        "addressLine1": "1 Independence Ave",
        "city": "Windhoek",
    })
}

#[tokio::test]
async fn a_payroll_operator_can_record_and_read_back_particulars() {
    let (cookie, employer_id) = an_operator_with_role(MembershipRole::PayrollOperator).await;
    let person_id = a_person(&employer_id, &cookie, "Ada Lovelace").await;

    let put_response = router()
        .await
        .oneshot(put_particulars_request(
            &employer_id,
            &person_id,
            &cookie,
            particulars_body(),
        ))
        .await
        .unwrap();
    assert_eq!(put_response.status(), StatusCode::OK);

    let get_response = router()
        .await
        .oneshot(get_request(&employer_id, &person_id, &cookie))
        .await
        .unwrap();
    assert_eq!(get_response.status(), StatusCode::OK);
    let json = body_json(get_response).await;
    assert_eq!(json["fullName"], "Ada Lovelace");
    assert_eq!(json["identityNumber"], "800101 5000 1");
    assert_eq!(json["addressLine1"], "1 Independence Ave");
    assert_eq!(json["city"], "Windhoek");
    assert!(
        json["particularsCreatedBy"]
            .as_str()
            .unwrap()
            .starts_with("operator:"),
        "the actor is always taken from the authorized context, never the request body: {json}"
    );
}

#[tokio::test]
async fn unrecorded_particulars_read_back_with_null_fields_but_the_full_name() {
    let (cookie, employer_id) = an_operator_with_role(MembershipRole::PayrollOperator).await;
    let person_id = a_person(&employer_id, &cookie, "Ada Lovelace").await;

    let response = router()
        .await
        .oneshot(get_request(&employer_id, &person_id, &cookie))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["fullName"], "Ada Lovelace");
    assert_eq!(json["identityNumber"], Value::Null);
    assert_eq!(json["actionLog"], json!([]));
}

#[tokio::test]
async fn a_blank_identity_number_is_refused() {
    let (cookie, employer_id) = an_operator_with_role(MembershipRole::PayrollOperator).await;
    let person_id = a_person(&employer_id, &cookie, "Ada Lovelace").await;

    let response = router()
        .await
        .oneshot(put_particulars_request(
            &employer_id,
            &person_id,
            &cookie,
            json!({ "identityNumber": "   ", "addressLine1": "1 Main St", "city": "Windhoek" }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "person_particulars_identity_number_cannot_be_empty"
    );
}

#[tokio::test]
async fn a_person_id_belonging_to_another_employer_is_refused_like_one_that_does_not_exist() {
    let (cookie, _own_employer_id) = an_operator_with_role(MembershipRole::PayrollOperator).await;
    let (other_cookie, other_employer_id) =
        an_operator_with_role(MembershipRole::PayrollOperator).await;
    let other_person_id = a_person(&other_employer_id, &other_cookie, "Ada Lovelace").await;

    let get_cross_employer = router()
        .await
        .oneshot(get_request(&other_employer_id, &other_person_id, &cookie))
        .await
        .unwrap();
    assert_eq!(get_cross_employer.status(), StatusCode::NOT_FOUND);

    let get_unknown_employer = router()
        .await
        .oneshot(get_request("no-such-employer", &other_person_id, &cookie))
        .await
        .unwrap();
    assert_eq!(get_unknown_employer.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(get_cross_employer).await["error"]["code"],
        body_json(get_unknown_employer).await["error"]["code"],
    );
}

#[tokio::test]
async fn a_person_id_of_another_employer_is_refused_on_write_and_writes_nothing() {
    let (cookie, own_employer_id) = an_operator_with_role(MembershipRole::PayrollOperator).await;
    let (other_cookie, other_employer_id) =
        an_operator_with_role(MembershipRole::PayrollOperator).await;
    let other_person_id = a_person(&other_employer_id, &other_cookie, "Ada Lovelace").await;

    // The Operator's own employer_id in the path, but the other Employer's
    // Person id — a caller cannot use one Employer's role to reach a Person
    // outside it just by naming its own employer_id.
    let put_own_employer = router()
        .await
        .oneshot(put_particulars_request(
            &own_employer_id,
            &other_person_id,
            &cookie,
            particulars_body(),
        ))
        .await
        .unwrap();
    assert_eq!(put_own_employer.status(), StatusCode::NOT_FOUND);

    let put_other_employer = router()
        .await
        .oneshot(put_particulars_request(
            &other_employer_id,
            &other_person_id,
            &cookie,
            particulars_body(),
        ))
        .await
        .unwrap();
    assert_eq!(put_other_employer.status(), StatusCode::NOT_FOUND);

    let get_after = router()
        .await
        .oneshot(get_request(
            &other_employer_id,
            &other_person_id,
            &other_cookie,
        ))
        .await
        .unwrap();
    assert_eq!(body_json(get_after).await["identityNumber"], Value::Null);
}

#[tokio::test]
async fn correcting_particulars_demands_a_reason_and_logs_it() {
    let (cookie, employer_id) = an_operator_with_role(MembershipRole::PayrollOperator).await;
    let person_id = a_person(&employer_id, &cookie, "Ada Lovelace").await;
    let first = router()
        .await
        .oneshot(put_particulars_request(
            &employer_id,
            &person_id,
            &cookie,
            particulars_body(),
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let mut correction = particulars_body();
    correction["identityNumber"] = json!("900101 5000 1");

    let unreasoned = router()
        .await
        .oneshot(put_particulars_request(
            &employer_id,
            &person_id,
            &cookie,
            correction.clone(),
        ))
        .await
        .unwrap();
    assert_eq!(unreasoned.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(unreasoned).await["error"]["code"],
        "person_particulars_correction_reason_cannot_be_empty"
    );

    correction["reason"] = json!("corrected a transposed digit");
    let reasoned = router()
        .await
        .oneshot(put_particulars_request(
            &employer_id,
            &person_id,
            &cookie,
            correction,
        ))
        .await
        .unwrap();
    assert_eq!(reasoned.status(), StatusCode::OK);

    let get_response = router()
        .await
        .oneshot(get_request(&employer_id, &person_id, &cookie))
        .await
        .unwrap();
    let json = body_json(get_response).await;
    assert_eq!(json["identityNumber"], "900101 5000 1");
    let entry = json["actionLog"][0].clone();
    assert_eq!(entry["actionType"], "person_particulars_corrected");
    assert_eq!(entry["context"]["reason"], "corrected a transposed digit");
}

#[tokio::test]
async fn a_name_correction_demands_a_reason_unconditionally_and_logs_it() {
    let (cookie, employer_id) = an_operator_with_role(MembershipRole::PayrollOperator).await;
    let person_id = a_person(&employer_id, &cookie, "Ada Lovelaec").await;

    let unreasoned = router()
        .await
        .oneshot(put_name_request(
            &employer_id,
            &person_id,
            &cookie,
            json!({ "fullName": "Ada Lovelace" }),
        ))
        .await
        .unwrap();
    assert_eq!(unreasoned.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(unreasoned).await["error"]["code"],
        "person_name_correction_reason_cannot_be_empty"
    );

    let reasoned = router()
        .await
        .oneshot(put_name_request(
            &employer_id,
            &person_id,
            &cookie,
            json!({ "fullName": "Ada Lovelace", "reason": "fixing a misspelling" }),
        ))
        .await
        .unwrap();
    assert_eq!(reasoned.status(), StatusCode::OK);

    let get_response = router()
        .await
        .oneshot(get_request(&employer_id, &person_id, &cookie))
        .await
        .unwrap();
    let json = body_json(get_response).await;
    assert_eq!(json["fullName"], "Ada Lovelace");
    let entry = json["actionLog"][0].clone();
    assert_eq!(entry["actionType"], "person_full_name_corrected");
    assert_eq!(entry["context"]["before"]["full_name"], "Ada Lovelaec");
    assert_eq!(entry["context"]["after"]["full_name"], "Ada Lovelace");
}
