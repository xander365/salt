//! Proves `POST /api/employers/{e}/employments`, `GET
//! /api/employers/{e}/employments` and `GET
//! /api/employers/{e}/employments/{em}` (issue #51, parent #49 Spec 2 of 3).
//! Driven with `tower::ServiceExt::oneshot` against the real router, the
//! same discipline `tests/employers.rs` already follows.

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
         (see crates/salt-server/tests/employments.rs)",
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

/// An Operator, logged in, with an active membership for a fresh Employer.
/// Returns the cookie and the Employer's own id.
async fn an_authorized_operator() -> (String, String) {
    let db = test_db().await;
    let email = unique_email("alice");
    let operator_id = create_operator(&email).await;
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
    let cookie = login(&email).await;
    (cookie, employer_id.to_string())
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn create_request(
    employer_id: &str,
    cookie: &str,
    salt_header: bool,
    body: Value,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!("/api/employers/{employer_id}/employments"))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if salt_header {
        builder = builder.header("x-salt-request", "1");
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

fn list_request(employer_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!("/api/employers/{employer_id}/employments"))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn detail_request(employer_id: &str, employment_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!(
            "/api/employers/{employer_id}/employments/{employment_id}"
        ))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn posting_without_the_salt_request_header_is_refused() {
    let (cookie, employer_id) = an_authorized_operator().await;

    let response = router()
        .await
        .oneshot(create_request(
            &employer_id,
            &cookie,
            false,
            serde_json::json!({ "fullName": "Ada Lovelace", "startDate": "2026-01-26" }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "salt_request_header_required");
}

#[tokio::test]
async fn both_person_id_and_full_name_is_malformed_request() {
    let (cookie, employer_id) = an_authorized_operator().await;

    let response = router()
        .await
        .oneshot(create_request(
            &employer_id,
            &cookie,
            true,
            serde_json::json!({
                "personId": "some-person",
                "fullName": "Ada Lovelace",
                "startDate": "2026-01-26",
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "malformed_request");
}

#[tokio::test]
async fn neither_person_id_nor_full_name_is_malformed_request() {
    let (cookie, employer_id) = an_authorized_operator().await;

    let response = router()
        .await
        .oneshot(create_request(
            &employer_id,
            &cookie,
            true,
            serde_json::json!({ "startDate": "2026-01-26" }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "malformed_request");
}

#[tokio::test]
async fn creating_by_full_name_creates_the_person_and_the_employment() {
    let (cookie, employer_id) = an_authorized_operator().await;

    let response = router()
        .await
        .oneshot(create_request(
            &employer_id,
            &cookie,
            true,
            serde_json::json!({ "fullName": "Ada Lovelace", "startDate": "2026-01-26" }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let created = body_json(response).await;
    let employment_id = created["employmentId"].as_str().unwrap().to_string();
    let person_id = created["personId"].as_str().unwrap().to_string();
    assert!(!employment_id.is_empty());
    assert!(!person_id.is_empty());

    let list_response = router()
        .await
        .oneshot(list_request(&employer_id, &cookie))
        .await
        .unwrap();
    assert_eq!(list_response.status(), StatusCode::OK);
    let listed = body_json(list_response).await;
    let employments = listed["employments"].as_array().unwrap();
    let listing = employments
        .iter()
        .find(|item| item["employmentId"] == employment_id)
        .expect("the created Employment is listed");
    assert_eq!(listing["fullName"], "Ada Lovelace");
    assert_eq!(listing["personId"], person_id);

    let detail_response = router()
        .await
        .oneshot(detail_request(&employer_id, &employment_id, &cookie))
        .await
        .unwrap();
    assert_eq!(detail_response.status(), StatusCode::OK);
    let detail = body_json(detail_response).await;
    assert_eq!(detail["fullName"], "Ada Lovelace");
    assert_eq!(detail["startDate"], "2026-01-26");
    assert_eq!(detail["endDate"], Value::Null);
    assert_eq!(detail["currentBasicPayCents"], Value::Null);
}

#[tokio::test]
async fn creating_by_an_existing_person_id_succeeds() {
    let (cookie, employer_id) = an_authorized_operator().await;

    let first = router()
        .await
        .oneshot(create_request(
            &employer_id,
            &cookie,
            true,
            serde_json::json!({ "fullName": "Ada Lovelace", "startDate": "2026-01-26" }),
        ))
        .await
        .unwrap();
    let person_id = body_json(first).await["personId"]
        .as_str()
        .unwrap()
        .to_string();

    let second = router()
        .await
        .oneshot(create_request(
            &employer_id,
            &cookie,
            true,
            serde_json::json!({ "personId": person_id, "startDate": "2026-07-26" }),
        ))
        .await
        .unwrap();

    assert_eq!(second.status(), StatusCode::OK);
    let created = body_json(second).await;
    assert_eq!(created["personId"], person_id);
}

#[tokio::test]
async fn a_person_id_belonging_to_another_employer_is_not_found() {
    let (owning_cookie, owning_employer) = an_authorized_operator().await;
    let (other_cookie, other_employer) = an_authorized_operator().await;

    let created = router()
        .await
        .oneshot(create_request(
            &owning_employer,
            &owning_cookie,
            true,
            serde_json::json!({ "fullName": "Ada Lovelace", "startDate": "2026-01-26" }),
        ))
        .await
        .unwrap();
    let person_id = body_json(created).await["personId"]
        .as_str()
        .unwrap()
        .to_string();

    let response = router()
        .await
        .oneshot(create_request(
            &other_employer,
            &other_cookie,
            true,
            serde_json::json!({ "personId": person_id, "startDate": "2026-01-26" }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "person_not_found");
}

#[tokio::test]
async fn an_employment_id_belonging_to_another_employer_is_not_found_on_the_detail_route() {
    let (owning_cookie, owning_employer) = an_authorized_operator().await;
    let (other_cookie, other_employer) = an_authorized_operator().await;

    let created = router()
        .await
        .oneshot(create_request(
            &owning_employer,
            &owning_cookie,
            true,
            serde_json::json!({ "fullName": "Ada Lovelace", "startDate": "2026-01-26" }),
        ))
        .await
        .unwrap();
    let employment_id = body_json(created).await["employmentId"]
        .as_str()
        .unwrap()
        .to_string();

    let response = router()
        .await
        .oneshot(detail_request(
            &other_employer,
            &employment_id,
            &other_cookie,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "employment_not_found");
}

#[tokio::test]
async fn an_unknown_employment_id_is_not_found_on_the_detail_route() {
    let (cookie, employer_id) = an_authorized_operator().await;

    let response = router()
        .await
        .oneshot(detail_request(&employer_id, "does-not-exist", &cookie))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "employment_not_found");
}

#[tokio::test]
async fn without_membership_the_routes_answer_404() {
    let (_, employer_id) = an_authorized_operator().await;
    let outsider_email = unique_email("mallory");
    create_operator(&outsider_email).await;
    let outsider_cookie = login(&outsider_email).await;

    let response = router()
        .await
        .oneshot(list_request(&employer_id, &outsider_cookie))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// A blank `fullName` reaches `payroll_app` — the "exactly one of" rule is
/// satisfied, so the handler cannot refuse it — and comes back as a 400 with
/// its own code, structurally the same envelope as a malformed body but
/// never the same code (§0.24).
#[tokio::test]
async fn a_blank_full_name_is_refused_as_a_bad_request() {
    let (cookie, employer_id) = an_authorized_operator().await;

    let response = router()
        .await
        .oneshot(create_request(
            &employer_id,
            &cookie,
            true,
            serde_json::json!({ "fullName": "   ", "startDate": "2026-01-26" }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "person_full_name_cannot_be_empty");
}

/// A name a form padded with whitespace is stored trimmed, so every screen
/// reads it the same way. The column is append-only, so getting this wrong
/// on write could not be corrected afterwards.
#[tokio::test]
async fn a_padded_full_name_is_stored_trimmed() {
    let (cookie, employer_id) = an_authorized_operator().await;

    let created = router()
        .await
        .oneshot(create_request(
            &employer_id,
            &cookie,
            true,
            serde_json::json!({ "fullName": "  Ada Lovelace  ", "startDate": "2026-01-26" }),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let employment_id = body_json(created).await["employmentId"]
        .as_str()
        .unwrap()
        .to_string();

    let detail = router()
        .await
        .oneshot(detail_request(&employer_id, &employment_id, &cookie))
        .await
        .unwrap();

    assert_eq!(body_json(detail).await["fullName"], "Ada Lovelace");
}

/// None of the three routes is reachable without a session (§0.24): an
/// unauthenticated request is 401, and never the 404 an authenticated
/// caller outside the Employer gets.
#[tokio::test]
async fn every_route_answers_401_without_a_session() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let created = router()
        .await
        .oneshot(create_request(
            &employer_id,
            &cookie,
            true,
            serde_json::json!({ "fullName": "Ada Lovelace", "startDate": "2026-01-26" }),
        ))
        .await
        .unwrap();
    let employment_id = body_json(created).await["employmentId"]
        .as_str()
        .unwrap()
        .to_string();

    let no_cookie = "";
    for request in [
        create_request(
            &employer_id,
            no_cookie,
            true,
            serde_json::json!({ "fullName": "Grace Hopper", "startDate": "2026-01-26" }),
        ),
        list_request(&employer_id, no_cookie),
        detail_request(&employer_id, &employment_id, no_cookie),
    ] {
        let uri = request.uri().clone();
        let method = request.method().clone();
        let response = router().await.oneshot(request).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {uri} must refuse an unauthenticated caller"
        );
    }
}
