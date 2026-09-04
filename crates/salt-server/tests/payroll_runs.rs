//! Proves `POST /api/employers/{e}/payroll-runs`, `GET
//! /api/employers/{e}/payroll-runs`, `GET
//! /api/employers/{e}/payroll-runs/{r}` and `PUT
//! /api/employers/{e}/payroll-runs/{r}/members/{em}/earnings` (issue #53,
//! parent #49 Spec 2 of 3). Driven with `tower::ServiceExt::oneshot` against
//! the real router, the same discipline `tests/employments.rs` already
//! follows.

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
         (see crates/salt-server/tests/payroll_runs.rs)",
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

/// An Operator, logged in, with an active membership for a fresh Employer on
/// a calendar-month `PaySchedule` (period end is the last day of the
/// month). Returns the cookie and the Employer's own id.
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

/// A month's own calendar `PayPeriod`: `2026-01-01` to `2026-01-31`, the
/// period `an_authorized_operator`'s `PaySchedule` generates.
fn january_period() -> Value {
    serde_json::json!({ "start": "2026-01-01", "end": "2026-01-31" })
}

fn create_employment_request(employer_id: &str, cookie: &str, full_name: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/api/employers/{employer_id}/employments"))
        .header(header::COOKIE, cookie)
        .header("x-salt-request", "1")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({ "fullName": full_name, "startDate": "2026-01-01" }).to_string(),
        ))
        .unwrap()
}

async fn create_employment(employer_id: &str, cookie: &str, full_name: &str) -> String {
    let response = router()
        .await
        .oneshot(create_employment_request(employer_id, cookie, full_name))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await["employmentId"]
        .as_str()
        .unwrap()
        .to_string()
}

fn create_run_request(
    employer_id: &str,
    cookie: &str,
    salt_header: bool,
    body: Value,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!("/api/employers/{employer_id}/payroll-runs"))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if salt_header {
        builder = builder.header("x-salt-request", "1");
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

async fn create_run(employer_id: &str, cookie: &str) -> String {
    let response = router()
        .await
        .oneshot(create_run_request(
            employer_id,
            cookie,
            true,
            serde_json::json!({ "period": january_period(), "payDate": "2026-02-05" }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await["payrollRunId"]
        .as_str()
        .unwrap()
        .to_string()
}

fn list_runs_request(employer_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!("/api/employers/{employer_id}/payroll-runs"))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn detail_request(employer_id: &str, run_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!("/api/employers/{employer_id}/payroll-runs/{run_id}"))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn set_earnings_request(
    employer_id: &str,
    run_id: &str,
    employment_id: &str,
    cookie: &str,
    salt_header: bool,
    body: Value,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("PUT")
        .uri(format!(
            "/api/employers/{employer_id}/payroll-runs/{run_id}/members/{employment_id}/earnings"
        ))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if salt_header {
        builder = builder.header("x-salt-request", "1");
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

#[tokio::test]
async fn posting_without_the_salt_request_header_is_refused() {
    let (cookie, employer_id) = an_authorized_operator().await;

    let response = router()
        .await
        .oneshot(create_run_request(
            &employer_id,
            &cookie,
            false,
            serde_json::json!({ "period": january_period(), "payDate": "2026-02-05" }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "salt_request_header_required");
}

#[tokio::test]
async fn creating_an_ordinary_run_proposes_every_overlapping_employment() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;

    let run_id = create_run(&employer_id, &cookie).await;
    assert!(!run_id.is_empty());

    let list_response = router()
        .await
        .oneshot(list_runs_request(&employer_id, &cookie))
        .await
        .unwrap();
    assert_eq!(list_response.status(), StatusCode::OK);
    let listed = body_json(list_response).await;
    let runs = listed["payrollRuns"].as_array().unwrap();
    let listing = runs
        .iter()
        .find(|item| item["payrollRunId"] == run_id)
        .expect("the created run is listed");
    assert_eq!(listing["status"], "draft");
    assert_eq!(listing["payDate"], "2026-02-05");
    assert_eq!(listing["period"]["start"], "2026-01-01");
    assert_eq!(listing["period"]["end"], "2026-01-31");

    let detail_response = router()
        .await
        .oneshot(detail_request(&employer_id, &run_id, &cookie))
        .await
        .unwrap();
    assert_eq!(detail_response.status(), StatusCode::OK);
    let detail = body_json(detail_response).await;
    assert_eq!(detail["status"], "draft");
    let members = detail["members"].as_array().unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["employmentId"], employment_id);
    assert_eq!(members[0]["fullName"], "Ada Lovelace");
    assert_eq!(members[0]["earnings"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn a_period_the_pay_schedule_does_not_generate_is_refused() {
    let (cookie, employer_id) = an_authorized_operator().await;

    let response = router()
        .await
        .oneshot(create_run_request(
            &employer_id,
            &cookie,
            true,
            serde_json::json!({
                "period": { "start": "2026-01-26", "end": "2026-02-25" },
                "payDate": "2026-03-05",
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(
        json["error"]["code"],
        "pay_period_not_generated_by_the_pay_schedule"
    );
}

#[tokio::test]
async fn setting_earnings_replaces_the_whole_list() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let run_id = create_run(&employer_id, &cookie).await;

    let first = router()
        .await
        .oneshot(set_earnings_request(
            &employer_id,
            &run_id,
            &employment_id,
            &cookie,
            true,
            serde_json::json!({
                "earnings": [
                    { "kind": "taxableAllowance", "amountCents": 5000 },
                    { "kind": "taxableAllowance", "amountCents": 2500 },
                ],
            }),
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);

    let after_first = body_json(
        router()
            .await
            .oneshot(detail_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    let earnings = after_first["members"][0]["earnings"].as_array().unwrap();
    assert_eq!(earnings.len(), 2);

    // Replaces, not merges: a second, shorter call leaves no stale lines.
    let second = router()
        .await
        .oneshot(set_earnings_request(
            &employer_id,
            &run_id,
            &employment_id,
            &cookie,
            true,
            serde_json::json!({
                "earnings": [{ "kind": "taxableAllowance", "amountCents": 1000 }],
            }),
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);

    let after_second = body_json(
        router()
            .await
            .oneshot(detail_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    let earnings = after_second["members"][0]["earnings"].as_array().unwrap();
    assert_eq!(earnings.len(), 1);
    assert_eq!(earnings[0]["kind"], "taxableAllowance");
    assert_eq!(earnings[0]["amountCents"], 1000);
}

#[tokio::test]
async fn setting_earnings_without_the_salt_request_header_is_refused() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let run_id = create_run(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(set_earnings_request(
            &employer_id,
            &run_id,
            &employment_id,
            &cookie,
            false,
            serde_json::json!({ "earnings": [] }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "salt_request_header_required");
}

/// `calculate` derives `BasicPay` itself from `CompensationTerms`, so a
/// `BasicPay` line in the request body reaches `set_run_earnings` and is
/// refused there (issue #53's own Deep Instructions) — never pre-checked or
/// silently dropped by the handler.
#[tokio::test]
async fn a_basic_pay_line_is_refused() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let run_id = create_run(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(set_earnings_request(
            &employer_id,
            &run_id,
            &employment_id,
            &cookie,
            true,
            serde_json::json!({
                "earnings": [{ "kind": "basicPay", "amountCents": 150000 }],
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "basic_pay_cannot_be_set_as_an_earning");
}

#[tokio::test]
async fn an_employment_that_is_not_an_active_member_is_refused() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let run_id = create_run(&employer_id, &cookie).await;
    // Never proposed: created after the run, so it never overlapped its
    // membership snapshot.
    let employment_id = create_employment(&employer_id, &cookie, "Grace Hopper").await;

    let response = router()
        .await
        .oneshot(set_earnings_request(
            &employer_id,
            &run_id,
            &employment_id,
            &cookie,
            true,
            serde_json::json!({ "earnings": [] }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "employment_not_an_active_run_member");
}

#[tokio::test]
async fn a_run_id_belonging_to_another_employer_is_not_found_on_the_detail_route() {
    let (owning_cookie, owning_employer) = an_authorized_operator().await;
    let (other_cookie, other_employer) = an_authorized_operator().await;
    let run_id = create_run(&owning_employer, &owning_cookie).await;

    let response = router()
        .await
        .oneshot(detail_request(&other_employer, &run_id, &other_cookie))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "payroll_run_not_found");
}

#[tokio::test]
async fn a_run_id_belonging_to_another_employer_is_not_found_on_the_earnings_route() {
    let (owning_cookie, owning_employer) = an_authorized_operator().await;
    let (other_cookie, other_employer) = an_authorized_operator().await;
    let employment_id = create_employment(&owning_employer, &owning_cookie, "Ada Lovelace").await;
    let run_id = create_run(&owning_employer, &owning_cookie).await;

    let response = router()
        .await
        .oneshot(set_earnings_request(
            &other_employer,
            &run_id,
            &employment_id,
            &other_cookie,
            true,
            serde_json::json!({ "earnings": [] }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "payroll_run_not_found");
}

#[tokio::test]
async fn an_unknown_run_id_is_not_found_on_the_detail_route() {
    let (cookie, employer_id) = an_authorized_operator().await;

    let response = router()
        .await
        .oneshot(detail_request(&employer_id, "does-not-exist", &cookie))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "payroll_run_not_found");
}

#[tokio::test]
async fn without_membership_the_routes_answer_404() {
    let (_, employer_id) = an_authorized_operator().await;
    let outsider_email = unique_email("mallory");
    create_operator(&outsider_email).await;
    let outsider_cookie = login(&outsider_email).await;

    let response = router()
        .await
        .oneshot(list_runs_request(&employer_id, &outsider_cookie))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn every_route_answers_401_without_a_session() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let run_id = create_run(&employer_id, &cookie).await;

    let no_cookie = "";
    for request in [
        create_run_request(
            &employer_id,
            no_cookie,
            true,
            serde_json::json!({ "period": january_period(), "payDate": "2026-02-05" }),
        ),
        list_runs_request(&employer_id, no_cookie),
        detail_request(&employer_id, &run_id, no_cookie),
        set_earnings_request(
            &employer_id,
            &run_id,
            &employment_id,
            no_cookie,
            true,
            serde_json::json!({ "earnings": [] }),
        ),
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
