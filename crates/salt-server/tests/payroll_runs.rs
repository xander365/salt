//! Proves `POST /api/employers/{e}/payroll-runs`, `GET
//! /api/employers/{e}/payroll-runs`, `GET
//! /api/employers/{e}/payroll-runs/{r}` and `PUT
//! /api/employers/{e}/payroll-runs/{r}/members/{em}/earnings` (issue #53,
//! parent #49 Spec 2 of 3), plus `POST
//! /api/employers/{e}/payroll-runs/{r}/calculate` (issue #55). Driven with
//! `tower::ServiceExt::oneshot` against the real router, the same
//! discipline `tests/employments.rs` already follows.

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
        .uri(format!(
            "/api/employers/{employer_id}/payroll-runs/{run_id}"
        ))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

/// `POST /api/employers/{e}/employments/{em}/unsupported-deductions`, the
/// route issue #52 already ships. Used here only to record the one standing
/// fact whose blocker carries `details`, so the wire shape of a
/// details-carrying blocker is proven end to end.
fn declare_unsupported_deductions_request(
    employer_id: &str,
    employment_id: &str,
    cookie: &str,
    body: Value,
) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!(
            "/api/employers/{employer_id}/employments/{employment_id}/unsupported-deductions"
        ))
        .header(header::COOKIE, cookie)
        .header("x-salt-request", "1")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
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

fn calculate_request(
    employer_id: &str,
    run_id: &str,
    cookie: &str,
    salt_header: bool,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/employers/{employer_id}/payroll-runs/{run_id}/calculate"
        ))
        .header(header::COOKIE, cookie);
    if salt_header {
        builder = builder.header("x-salt-request", "1");
    }
    builder.body(Body::empty()).unwrap()
}

/// `POST /api/employers/{e}/employments/{em}/compensation-terms`, the route
/// issue #52 already ships. Used here to give a member the one fact
/// `calculate` reads first, so a member missing only this one is the
/// minimal way to prove a per-member `refusal` on the calculate route.
fn record_compensation_terms_request(
    employer_id: &str,
    employment_id: &str,
    cookie: &str,
    effective_from: &str,
    basic_pay_cents: i64,
) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!(
            "/api/employers/{employer_id}/employments/{employment_id}/compensation-terms"
        ))
        .header(header::COOKIE, cookie)
        .header("x-salt-request", "1")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({
                "effectiveFrom": effective_from,
                "basicPayCents": basic_pay_cents,
            })
            .to_string(),
        ))
        .unwrap()
}

/// `POST /api/employers/{e}/employments/{em}/prior-employment`, the route
/// issue #52 already ships. Used here only to declare `confirmed_none`, so
/// a fully-declared member has no standing blocker left.
fn declare_prior_employment_request(
    employer_id: &str,
    employment_id: &str,
    cookie: &str,
    tax_year: i32,
) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!(
            "/api/employers/{employer_id}/employments/{employment_id}/prior-employment"
        ))
        .header(header::COOKIE, cookie)
        .header("x-salt-request", "1")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({
                "taxYear": tax_year,
                "status": "confirmed_none",
            })
            .to_string(),
        ))
        .unwrap()
}

/// Declares every fact `calculate` needs for `employment_id` under
/// `january_period()`'s own `PaySchedule` and `TaxYear` — `CompensationTerms`
/// effective from the start of that period, a confirmed absence of
/// `PriorEmployment` for the TaxYear `january_period()`'s end falls in, and
/// a confirmed absence of unsupported deductions. Ready to calculate the
/// instant it is a run member.
async fn fully_declare_employment(employer_id: &str, employment_id: &str, cookie: &str) {
    let response = router()
        .await
        .oneshot(record_compensation_terms_request(
            employer_id,
            employment_id,
            cookie,
            "2026-01-01",
            1_500_000,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // 2026-01-31 (january_period()'s own end) falls in the TaxYear starting
    // 2025 (1 March 2025 - end of February 2026).
    let response = router()
        .await
        .oneshot(declare_prior_employment_request(
            employer_id,
            employment_id,
            cookie,
            2025,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = router()
        .await
        .oneshot(declare_unsupported_deductions_request(
            employer_id,
            employment_id,
            cookie,
            serde_json::json!({
                "effectiveFrom": "2026-01-01",
                "status": "confirmed_none",
                "reason": "no unsupported deductions",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
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

    // No standing-data screen in this ticket's scope has declared anything
    // for this Employment yet (issue #54, §0.31): every fact a read alone
    // can judge is still missing, so the wire response names all three.
    let blockers = members[0]["blockers"].as_array().unwrap();
    let codes: Vec<&str> = blockers
        .iter()
        .map(|blocker| blocker["code"].as_str().unwrap())
        .collect();
    assert_eq!(
        codes,
        vec![
            "prior_employment_unknown",
            "unsupported_deduction_status_unknown",
            "no_compensation_terms_in_force",
        ]
    );
    assert!(blockers.iter().all(|blocker| blocker["details"].is_null()));
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
    assert_eq!(
        json["error"]["code"],
        "basic_pay_cannot_be_set_as_an_earning"
    );
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

/// A signed-in Operator with no membership for this Employer gets 404 on
/// every one of the four routes, not 403: 403 would confirm the Employer
/// exists, which is a free existence oracle over payroll (ADR-0017). The
/// `POST` is in the loop because a non-member creating a run would be a
/// write, not merely a read.
#[tokio::test]
async fn without_membership_the_routes_answer_404() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let run_id = create_run(&employer_id, &cookie).await;
    let outsider_email = unique_email("mallory");
    create_operator(&outsider_email).await;
    let outsider_cookie = login(&outsider_email).await;

    for request in [
        create_run_request(
            &employer_id,
            &outsider_cookie,
            true,
            serde_json::json!({
                "period": { "start": "2026-02-01", "end": "2026-02-28" },
                "payDate": "2026-03-05",
            }),
        ),
        list_runs_request(&employer_id, &outsider_cookie),
        detail_request(&employer_id, &run_id, &outsider_cookie),
        set_earnings_request(
            &employer_id,
            &run_id,
            &employment_id,
            &outsider_cookie,
            true,
            serde_json::json!({ "earnings": [] }),
        ),
        calculate_request(&employer_id, &run_id, &outsider_cookie, true),
    ] {
        let uri = request.uri().clone();
        let method = request.method().clone();
        let response = router().await.oneshot(request).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{method} {uri} must 404 a non-member"
        );
    }

    // The non-member's POST wrote nothing: the owner still has exactly the
    // one run they created.
    let listed = body_json(
        router()
            .await
            .oneshot(list_runs_request(&employer_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(listed["payrollRuns"].as_array().unwrap().len(), 1);
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
        calculate_request(&employer_id, &run_id, no_cookie, true),
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

/// Acceptance: a run belonging to another Employer is absent from the list
/// route, not merely 404 on the detail one. `.find()` in the happy-path test
/// above cannot prove an absence, so this one asserts it directly.
#[tokio::test]
async fn another_employers_run_is_not_in_the_list() {
    let (owning_cookie, owning_employer) = an_authorized_operator().await;
    let (other_cookie, other_employer) = an_authorized_operator().await;
    let their_run = create_run(&owning_employer, &owning_cookie).await;
    let our_run = create_run(&other_employer, &other_cookie).await;

    let response = router()
        .await
        .oneshot(list_runs_request(&other_employer, &other_cookie))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let listed: Vec<&str> = json["payrollRuns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|run| run["payrollRunId"].as_str().unwrap())
        .collect();
    assert_eq!(listed, vec![our_run.as_str()]);
    assert!(!listed.contains(&their_run.as_str()));
}

/// §0.24: "you typed something wrong" is 400 and structurally different from
/// the 422 a payroll refusal answers with — never the same shape. The two
/// bodies below are malformed in the two ways a client actually gets wrong:
/// a missing field and a period whose dates are not dates.
#[tokio::test]
async fn a_malformed_create_body_is_a_bad_request_not_a_refusal() {
    let (cookie, employer_id) = an_authorized_operator().await;

    for body in [
        serde_json::json!({ "period": january_period() }),
        serde_json::json!({ "period": january_period(), "payDate": "not-a-date" }),
        serde_json::json!({
            "period": { "start": "2026-01-31", "end": "2026-01-01" },
            "payDate": "2026-02-05",
        }),
    ] {
        let response = router()
            .await
            .oneshot(create_run_request(
                &employer_id,
                &cookie,
                true,
                body.clone(),
            ))
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{body} must be a malformed request, not a payroll refusal"
        );
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "malformed_request");
    }
}

/// An Earning line Salt cannot represent is a transport-shape failure, not a
/// payroll refusal: a `kind` no `Earning` variant spells, and an amount
/// `Money` refuses because money is never negative (INV-001 reaches the
/// boundary too).
#[tokio::test]
async fn an_earning_line_salt_cannot_represent_is_a_bad_request() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let run_id = create_run(&employer_id, &cookie).await;

    for body in [
        serde_json::json!({ "earnings": [{ "kind": "bonus", "amountCents": 1000 }] }),
        serde_json::json!({ "earnings": [{ "kind": "taxableAllowance", "amountCents": -1 }] }),
        serde_json::json!({ "earnings": [{ "kind": "taxableAllowance" }] }),
    ] {
        let response = router()
            .await
            .oneshot(set_earnings_request(
                &employer_id,
                &run_id,
                &employment_id,
                &cookie,
                true,
                body.clone(),
            ))
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{body} must be a malformed request, not a payroll refusal"
        );
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "malformed_request");
    }
}

/// A malformed Earning line is refused whole: nothing in the same request is
/// written first. Without this the `PUT`'s "replaces the whole list" promise
/// would hold only for requests that happen to parse.
#[tokio::test]
async fn a_malformed_earning_line_leaves_the_existing_lines_alone() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let run_id = create_run(&employer_id, &cookie).await;
    let accepted = router()
        .await
        .oneshot(set_earnings_request(
            &employer_id,
            &run_id,
            &employment_id,
            &cookie,
            true,
            serde_json::json!({
                "earnings": [{ "kind": "taxableAllowance", "amountCents": 5000 }],
            }),
        ))
        .await
        .unwrap();
    assert_eq!(accepted.status(), StatusCode::OK);

    let refused = router()
        .await
        .oneshot(set_earnings_request(
            &employer_id,
            &run_id,
            &employment_id,
            &cookie,
            true,
            serde_json::json!({
                "earnings": [
                    { "kind": "taxableAllowance", "amountCents": 1000 },
                    { "kind": "bonus", "amountCents": 1000 },
                ],
            }),
        ))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);

    let detail = body_json(
        router()
            .await
            .oneshot(detail_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    let earnings = detail["members"][0]["earnings"].as_array().unwrap();
    assert_eq!(earnings.len(), 1);
    assert_eq!(earnings[0]["amountCents"], 5000);
}

/// An empty `earnings` list is a complete statement — no additional
/// Earnings this period — and clears whatever was there, rather than being
/// ignored as "nothing to do". It is the `PUT`'s replacement promise at its
/// shortest.
#[tokio::test]
async fn setting_no_earnings_at_all_clears_the_list() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let run_id = create_run(&employer_id, &cookie).await;
    router()
        .await
        .oneshot(set_earnings_request(
            &employer_id,
            &run_id,
            &employment_id,
            &cookie,
            true,
            serde_json::json!({
                "earnings": [{ "kind": "taxableAllowance", "amountCents": 5000 }],
            }),
        ))
        .await
        .unwrap();

    let cleared = router()
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
    assert_eq!(cleared.status(), StatusCode::OK);

    let detail = body_json(
        router()
            .await
            .oneshot(detail_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        detail["members"][0]["earnings"].as_array().unwrap().len(),
        0
    );
}

/// A body naming another Employer's id changes nothing: scope comes from the
/// URL and the session alone (§0.22, user story 39). The run created below
/// belongs to the caller's own Employer.
#[tokio::test]
async fn an_employer_id_in_the_body_is_ignored() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let (_other_cookie, other_employer) = an_authorized_operator().await;

    let response = router()
        .await
        .oneshot(create_run_request(
            &employer_id,
            &cookie,
            true,
            serde_json::json!({
                "period": january_period(),
                "payDate": "2026-02-05",
                "employerId": other_employer,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let run_id = body_json(response).await["payrollRunId"]
        .as_str()
        .unwrap()
        .to_string();

    let ours = router()
        .await
        .oneshot(detail_request(&employer_id, &run_id, &cookie))
        .await
        .unwrap();
    assert_eq!(ours.status(), StatusCode::OK);
}

/// A `PayrollOperator`, not only an `Owner`, is let through all four routes
/// (§0.6, parent #49: none of this spec's routes is Owner-only).
#[tokio::test]
async fn a_payroll_operator_reaches_every_route() {
    let db = test_db().await;
    let email = unique_email("bob");
    let operator_id = create_operator(&email).await;
    let employer_id = payroll_app::create_employer(
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
        &employer_id,
        MembershipRole::PayrollOperator,
    )
    .await
    .unwrap();
    let cookie = login(&email).await;
    let employer_id = employer_id.to_string();
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let run_id = create_run(&employer_id, &cookie).await;

    for request in [
        list_runs_request(&employer_id, &cookie),
        detail_request(&employer_id, &run_id, &cookie),
        set_earnings_request(
            &employer_id,
            &run_id,
            &employment_id,
            &cookie,
            true,
            serde_json::json!({ "earnings": [] }),
        ),
        calculate_request(&employer_id, &run_id, &cookie, true),
    ] {
        let uri = request.uri().clone();
        let method = request.method().clone();
        let response = router().await.oneshot(request).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{method} {uri} must be reachable by a PayrollOperator"
        );
    }
}

/// `unsupported_deductions_present` is the one blocker whose `details` a
/// screen must read: "you have unsupported deductions" without naming them
/// is unactionable (issue #54, §0.31). This proves the kinds survive the
/// whole path — declaration route, read model, DTO — under the same code
/// the refusal itself uses.
#[tokio::test]
async fn a_present_unsupported_deduction_blocker_names_its_kinds_on_the_wire() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;

    let declared = router()
        .await
        .oneshot(declare_unsupported_deductions_request(
            &employer_id,
            &employment_id,
            &cookie,
            serde_json::json!({
                "effectiveFrom": "2026-01-01",
                "status": "present",
                "kinds": ["provident_fund", "education_policy"],
                "acknowledgedDivergingPeriods": [],
                "reason": "joined a provident fund",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(declared.status(), StatusCode::OK);

    let run_id = create_run(&employer_id, &cookie).await;
    let response = router()
        .await
        .oneshot(detail_request(&employer_id, &run_id, &cookie))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let detail = body_json(response).await;
    let blockers = detail["members"][0]["blockers"].as_array().unwrap();
    let present = blockers
        .iter()
        .find(|blocker| blocker["code"] == "unsupported_deductions_present")
        .expect("a present declaration blocks under its own code");
    assert_eq!(
        present["details"]["kinds"],
        serde_json::json!(["provident_fund", "education_policy"])
    );
    assert!(
        !blockers
            .iter()
            .any(|blocker| blocker["code"] == "unsupported_deduction_status_unknown"),
        "a declaration in force is never also reported as unknown"
    );
}

/// Issue #55's central acceptance criterion: a fully-declared member gets
/// its figures back, cents-exact, and the run's own `status` says Finalize
/// is now allowed.
#[tokio::test]
async fn calculating_a_fully_declared_run_returns_figures_and_the_run_becomes_calculated() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    fully_declare_employment(&employer_id, &employment_id, &cookie).await;
    let run_id = create_run(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(calculate_request(&employer_id, &run_id, &cookie, true))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let detail = body_json(response).await;
    assert_eq!(detail["status"], "calculated");
    let member = &detail["members"][0];
    assert!(member["refusal"].is_null());
    let figures = &member["figures"];
    assert_eq!(figures["basicPayCents"], 1_500_000);
    assert_eq!(figures["taxableAllowancesCents"], 0);
    assert_eq!(figures["grossCents"], 1_500_000);
    assert_eq!(figures["taxableRemunerationCents"], 1_500_000);
    assert!(figures["payeCents"].is_i64());
    assert!(figures["employeeSscCents"].is_i64());
    assert!(figures["employerSscCents"].is_i64());
    assert!(figures["netCents"].is_i64());

    // A plain refresh reads the same figures back — the stored
    // `WorkingPayrollCalculation`, not a claim only Calculate's own
    // response can make.
    let refreshed = body_json(
        router()
            .await
            .oneshot(detail_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(refreshed["status"], "calculated");
    assert_eq!(refreshed["members"][0]["figures"], *figures);
    assert!(refreshed["members"][0]["refusal"].is_null());
}

/// A member missing its `CompensationTerms` cannot calculate — but the
/// route still answers 200, with the reason in that member's own
/// `refusal`, under the same stable code the error envelope itself would
/// use (§0.25, issue #55's own Deep Instructions). With one member, "the
/// run refused" and "every member refused" are the same run, so this also
/// proves that acceptance criterion.
#[tokio::test]
async fn calculating_a_blocked_member_returns_200_with_its_own_refusal() {
    let (cookie, employer_id) = an_authorized_operator().await;
    create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let run_id = create_run(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(calculate_request(&employer_id, &run_id, &cookie, true))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a fully-refused run is still 200 (§0.25): Calculate is not an error"
    );
    let detail = body_json(response).await;
    assert_eq!(detail["status"], "draft");
    let member = &detail["members"][0];
    assert!(member["figures"].is_null());
    assert_eq!(member["refusal"]["code"], "no_compensation_terms_in_force");

    // A plain refresh never recomputes: figures stay absent, but so does
    // `refusal` — it is never persisted, unlike `blockers`.
    let refreshed = body_json(
        router()
            .await
            .oneshot(detail_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert!(refreshed["members"][0]["refusal"].is_null());
}

/// A partial calculation is still a successful response: the ready member's
/// figures must not be hidden by another member's refusal (#49 §0.25).
#[tokio::test]
async fn calculating_a_partially_blocked_run_returns_figures_and_a_member_refusal() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let ready_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    fully_declare_employment(&employer_id, &ready_id, &cookie).await;
    let blocked_id = create_employment(&employer_id, &cookie, "Grace Hopper").await;
    let run_id = create_run(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(calculate_request(&employer_id, &run_id, &cookie, true))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let detail = body_json(response).await;
    assert_eq!(detail["status"], "draft");
    let members = detail["members"].as_array().unwrap();
    let ready = members
        .iter()
        .find(|member| member["employmentId"] == ready_id)
        .unwrap();
    assert_eq!(ready["figures"]["basicPayCents"], 1_500_000);
    assert!(ready["refusal"].is_null());
    let blocked = members
        .iter()
        .find(|member| member["employmentId"] == blocked_id)
        .unwrap();
    assert!(blocked["figures"].is_null());
    assert_eq!(blocked["refusal"]["code"], "no_compensation_terms_in_force");
}

#[tokio::test]
async fn the_calculate_route_requires_the_salt_request_header() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let run_id = create_run(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(calculate_request(&employer_id, &run_id, &cookie, false))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "salt_request_header_required");
}

#[tokio::test]
async fn a_run_id_belonging_to_another_employer_is_not_found_on_the_calculate_route() {
    let (owning_cookie, owning_employer) = an_authorized_operator().await;
    let (other_cookie, other_employer) = an_authorized_operator().await;
    let run_id = create_run(&owning_employer, &owning_cookie).await;

    let response = router()
        .await
        .oneshot(calculate_request(
            &other_employer,
            &run_id,
            &other_cookie,
            true,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "payroll_run_not_found");
}

/// A retried Calculate against an already-`Finalized` run is refused with
/// its own stable code (issue #55's own acceptance criterion), the same 409
/// a retried Finalize itself gets (§0.28) — Calculate shares the refusal,
/// not the DTO shape, with that outcome.
#[tokio::test]
async fn calculating_an_already_finalized_run_is_refused_with_its_stable_code() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    fully_declare_employment(&employer_id, &employment_id, &cookie).await;
    let run_id = create_run(&employer_id, &cookie).await;

    let calculated = router()
        .await
        .oneshot(calculate_request(&employer_id, &run_id, &cookie, true))
        .await
        .unwrap();
    assert_eq!(calculated.status(), StatusCode::OK);

    // Finalization has no HTTP route yet (a later ticket's own scope), so
    // the run is finalized directly through `payroll_app` here.
    let db = test_db().await;
    let domain_employer_id = payroll::EmployerId::new(employer_id.clone());
    let payroll_run_id =
        payroll_app::verify_payroll_run_belongs_to_employer(&db, &domain_employer_id, &run_id)
            .await
            .unwrap();
    payroll_app::finalize_payroll_run(&db, &payroll_run_id, "test-setup")
        .await
        .unwrap();

    let response = router()
        .await
        .oneshot(calculate_request(&employer_id, &run_id, &cookie, true))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "payroll_run_already_finalized");
}
