//! Proves issue #58's own acceptance criteria (parent #49 Spec 2 of 3): one
//! test drives a whole ordinary payroll through the real router, and a set
//! of tests proves that ids from another Employer are not keys and that a
//! typo is never mistaken for a payroll refusal. Actor integrity is proven
//! in `crates/payroll-app/tests/http_actor_integrity.rs`, where the existing
//! SQLx harness can inspect the HTTP-written ActionLog without giving
//! salt-server SQL access (ADR-0018).
//! Driven with `tower::ServiceExt::oneshot` against the
//! real router, the same discipline every other file under this directory
//! already follows — no TCP port, no browser.
//!
//! Every precondition here is built through the public routes, the same way
//! `tests/payroll_runs.rs` and `tests/finalized_payroll.rs` already do.
//! Creating an Operator, an Employer or a membership has no route in Specs
//! 1–3 (§0.39) — the one exception the issue's own Deep Instructions name —
//! so those, and only those, go through `payroll_app` directly.
//!
//! Three tests here claim to walk *every* route, from lists written by
//! hand. `the_declared_route_table_is_exactly_the_one_these_tests_walk`
//! keeps those claims honest: it reads `src/router.rs` at compile time and
//! fails if the declared table ever stops being §0.22's own.

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
         (see crates/salt-server/tests/payroll_journey.rs)",
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
                    json!({
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

/// An Operator, logged in, with an active `role` membership for a fresh
/// Employer on a calendar-month `PaySchedule` (period end is the last day of
/// the month). Returns the operator's own id, the cookie and the Employer's
/// own id.
async fn an_authorized_operator(role: MembershipRole) -> (OperatorId, String, String) {
    let db = test_db().await;
    let email = unique_email("operator");
    let operator_id = create_operator(&email).await;
    let employer_id = payroll_app::create_employer(
        &db,
        "Acme Corp",
        payroll::PaySchedule::new(payroll::PeriodEndDay::LastDayOfMonth),
        "test-setup",
    )
    .await
    .unwrap();
    payroll_app::create_employer_membership(&db, &operator_id, &employer_id, role)
        .await
        .unwrap();
    let cookie = login(&email).await;
    (operator_id, cookie, employer_id.to_string())
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// A month's own calendar `PayPeriod`: `2026-01-01` to `2026-01-31`, the
/// period `an_authorized_operator`'s own `PaySchedule` generates.
fn january_period() -> Value {
    json!({ "start": "2026-01-01", "end": "2026-01-31" })
}

// ---------------------------------------------------------------------
// Request builders — one per Spec 2 route.
// ---------------------------------------------------------------------

fn get_employers_request(cookie: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method("GET").uri("/api/employers");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    builder.body(Body::empty()).unwrap()
}

fn create_employment_request(
    employer_id: &str,
    cookie: &str,
    salt_header: bool,
    full_name: &str,
    start_date: &str,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!("/api/employers/{employer_id}/employments"))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if salt_header {
        builder = builder.header("x-salt-request", "1");
    }
    builder
        .body(Body::from(
            json!({ "fullName": full_name, "startDate": start_date }).to_string(),
        ))
        .unwrap()
}

async fn create_employment(employer_id: &str, cookie: &str, full_name: &str) -> String {
    create_employment_starting(employer_id, cookie, full_name, "2026-01-01").await
}

async fn create_employment_starting(
    employer_id: &str,
    cookie: &str,
    full_name: &str,
    start_date: &str,
) -> String {
    let response = router()
        .await
        .oneshot(create_employment_request(
            employer_id,
            cookie,
            true,
            full_name,
            start_date,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await["employmentId"]
        .as_str()
        .unwrap()
        .to_string()
}

fn list_employments_request(employer_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!("/api/employers/{employer_id}/employments"))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn get_employer_particulars_request(employer_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!("/api/employers/{employer_id}/particulars"))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn set_employer_particulars_request(
    employer_id: &str,
    cookie: &str,
    salt_header: bool,
    body: Value,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("PUT")
        .uri(format!("/api/employers/{employer_id}/particulars"))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if salt_header {
        builder = builder.header("x-salt-request", "1");
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

fn get_person_particulars_request(
    employer_id: &str,
    person_id: &str,
    cookie: &str,
) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!(
            "/api/employers/{employer_id}/people/{person_id}/particulars"
        ))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn set_person_particulars_request(
    employer_id: &str,
    person_id: &str,
    cookie: &str,
    salt_header: bool,
    body: Value,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("PUT")
        .uri(format!(
            "/api/employers/{employer_id}/people/{person_id}/particulars"
        ))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if salt_header {
        builder = builder.header("x-salt-request", "1");
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

fn correct_person_full_name_request(
    employer_id: &str,
    person_id: &str,
    cookie: &str,
    salt_header: bool,
    body: Value,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("PUT")
        .uri(format!(
            "/api/employers/{employer_id}/people/{person_id}/name"
        ))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if salt_header {
        builder = builder.header("x-salt-request", "1");
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

fn employment_detail_request(
    employer_id: &str,
    employment_id: &str,
    cookie: &str,
) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!(
            "/api/employers/{employer_id}/employments/{employment_id}"
        ))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn record_compensation_terms_request(
    employer_id: &str,
    employment_id: &str,
    cookie: &str,
    salt_header: bool,
    body: Value,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/employers/{employer_id}/employments/{employment_id}/compensation-terms"
        ))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if salt_header {
        builder = builder.header("x-salt-request", "1");
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

fn record_employment_end_date_request(
    employer_id: &str,
    employment_id: &str,
    cookie: &str,
    salt_header: bool,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("PUT")
        .uri(format!(
            "/api/employers/{employer_id}/employments/{employment_id}/end-date"
        ))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if salt_header {
        builder = builder.header("x-salt-request", "1");
    }
    builder
        .body(Body::from(
            json!({ "endDate": "2026-06-25", "reason": "resigned" }).to_string(),
        ))
        .unwrap()
}

fn declare_prior_employment_request(
    employer_id: &str,
    employment_id: &str,
    cookie: &str,
    salt_header: bool,
    tax_year: i32,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/employers/{employer_id}/employments/{employment_id}/prior-employment"
        ))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if salt_header {
        builder = builder.header("x-salt-request", "1");
    }
    builder
        .body(Body::from(
            json!({ "taxYear": tax_year, "status": "confirmed_none" }).to_string(),
        ))
        .unwrap()
}

fn declare_unsupported_deductions_request(
    employer_id: &str,
    employment_id: &str,
    cookie: &str,
    salt_header: bool,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/employers/{employer_id}/employments/{employment_id}/unsupported-deductions"
        ))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if salt_header {
        builder = builder.header("x-salt-request", "1");
    }
    builder
        .body(Body::from(
            json!({
                "effectiveFrom": "2026-01-01",
                "status": "confirmed_none",
                "reason": "no unsupported deductions",
            })
            .to_string(),
        ))
        .unwrap()
}

fn record_opening_balance_request(
    employer_id: &str,
    employment_id: &str,
    cookie: &str,
    salt_header: bool,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/employers/{employer_id}/employments/{employment_id}/opening-balance"
        ))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if salt_header {
        builder = builder.header("x-salt-request", "1");
    }
    builder
        .body(Body::from(
            json!({
                "taxYear": 2025,
                "saltCoverageStart": "2026-01-31",
                "priorTaxableRemunerationCents": 3_000_000_i64,
                "priorPayeCents": 0,
            })
            .to_string(),
        ))
        .unwrap()
}

fn list_standing_pay_items_request(
    employer_id: &str,
    employment_id: &str,
    cookie: &str,
) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!(
            "/api/employers/{employer_id}/employments/{employment_id}/standing-pay-items"
        ))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn create_standing_pay_item_request(
    employer_id: &str,
    employment_id: &str,
    cookie: &str,
    salt_header: bool,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/employers/{employer_id}/employments/{employment_id}/standing-pay-items"
        ))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if salt_header {
        builder = builder.header("x-salt-request", "1");
    }
    builder
        .body(Body::from(
            json!({
                "kind": "taxableAllowance",
                "effectiveFrom": "2026-01-01",
                "amountCents": 10_000,
                "label": "standby",
            })
            .to_string(),
        ))
        .unwrap()
}

fn end_standing_pay_item_request(
    employer_id: &str,
    employment_id: &str,
    standing_pay_item_id: &str,
    cookie: &str,
    salt_header: bool,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/employers/{employer_id}/employments/{employment_id}/standing-pay-items/{standing_pay_item_id}/end"
        ))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if salt_header {
        builder = builder.header("x-salt-request", "1");
    }
    builder
        .body(Body::from(
            json!({ "reason": "no longer applies" }).to_string(),
        ))
        .unwrap()
}

/// Declares every fact `calculate` needs for `employment_id` under
/// `january_period()`'s own `PaySchedule` and `TaxYear`: `CompensationTerms`
/// effective from the start of that period, and a confirmed absence of
/// `PriorEmployment` and unsupported deductions for the TaxYear
/// `january_period()`'s end falls in.
async fn fully_declare_employment(employer_id: &str, employment_id: &str, cookie: &str) {
    let response = router()
        .await
        .oneshot(record_compensation_terms_request(
            employer_id,
            employment_id,
            cookie,
            true,
            json!({
                "effectiveFrom": "2026-01-01",
                "basicPayCents": 1_500_000_i64,
                "ordinaryHours": "40.00",
            }),
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
            true,
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
            true,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
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
            json!({ "period": january_period(), "payDate": "2026-02-05" }),
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

fn run_detail_request(employer_id: &str, run_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!(
            "/api/employers/{employer_id}/payroll-runs/{run_id}"
        ))
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
            "/api/employers/{employer_id}/payroll-runs/{run_id}/members/{employment_id}/pay-lines"
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

fn finalize_request(
    employer_id: &str,
    run_id: &str,
    cookie: &str,
    salt_header: bool,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/employers/{employer_id}/payroll-runs/{run_id}/finalize"
        ))
        .header(header::COOKIE, cookie);
    if salt_header {
        builder = builder.header("x-salt-request", "1");
    }
    builder.body(Body::empty()).unwrap()
}

fn finalized_detail_request(
    employer_id: &str,
    finalized_payroll_id: &str,
    cookie: &str,
) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!(
            "/api/employers/{employer_id}/finalized-payroll/{finalized_payroll_id}"
        ))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn finalized_traces_request(
    employer_id: &str,
    finalized_payroll_id: &str,
    cookie: &str,
) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!(
            "/api/employers/{employer_id}/finalized-payroll/{finalized_payroll_id}/traces"
        ))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

/// `POST .../pay-lines/{standing_pay_item_id}/override` (issue #80).
fn override_standing_pay_line_request(
    employer_id: &str,
    run_id: &str,
    employment_id: &str,
    standing_pay_item_id: &str,
    cookie: &str,
    salt_header: bool,
    amount_cents: i64,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/employers/{employer_id}/payroll-runs/{run_id}/members/{employment_id}/pay-lines/{standing_pay_item_id}/override"
        ))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if salt_header {
        builder = builder.header("x-salt-request", "1");
    }
    builder
        .body(Body::from(
            json!({
                "line": {
                    "kind": "taxableAllowance",
                    "amountCents": amount_cents,
                    "label": "standby",
                },
                "reason": "temporary raise",
            })
            .to_string(),
        ))
        .unwrap()
}

/// `POST .../pay-lines/{standing_pay_item_id}/remove` (issue #80).
fn remove_standing_pay_line_request(
    employer_id: &str,
    run_id: &str,
    employment_id: &str,
    standing_pay_item_id: &str,
    cookie: &str,
    salt_header: bool,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/employers/{employer_id}/payroll-runs/{run_id}/members/{employment_id}/pay-lines/{standing_pay_item_id}/remove"
        ))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if salt_header {
        builder = builder.header("x-salt-request", "1");
    }
    builder
        .body(Body::from(
            json!({ "reason": "not needed this run" }).to_string(),
        ))
        .unwrap()
}

/// `POST .../refresh-proposals` (issue #80).
fn refresh_proposals_request(
    employer_id: &str,
    run_id: &str,
    cookie: &str,
    salt_header: bool,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/employers/{employer_id}/payroll-runs/{run_id}/refresh-proposals"
        ))
        .header(header::COOKIE, cookie);
    if salt_header {
        builder = builder.header("x-salt-request", "1");
    }
    builder.body(Body::empty()).unwrap()
}

// ---------------------------------------------------------------------
// The tracer bullet.
// ---------------------------------------------------------------------

/// Acceptance: the whole ordinary payroll journey, in one place — sign in,
/// create an Employment by name, record CompensationTerms, both
/// declarations, create an Ordinary run, read its members, set a taxable
/// allowance, calculate, read the ten figures, finalize, read the
/// finalized payroll back, read its traces. Deliberately one test, not a
/// suite (issue #58's own Deep Instructions): its whole value is that the
/// entire journey lives in one place.
#[tokio::test]
async fn signing_in_and_running_one_ordinary_payroll_end_to_end() {
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
    let employer_id = employer_id.to_string();

    // Sign in.
    let login_response = router()
        .await
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/session")
                .header("x-salt-request", "1")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    json!({
                        "email": email,
                        "password": "correct horse battery staple",
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(login_response.status(), StatusCode::OK);
    let cookie = login_response
        .headers()
        .get(header::SET_COOKIE)
        .expect("a successful login always sets the session cookie")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    // Create an Employment by name.
    let created = router()
        .await
        .oneshot(create_employment_request(
            &employer_id,
            &cookie,
            true,
            "Ada Lovelace",
            "2026-01-01",
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let employment_id = body_json(created).await["employmentId"]
        .as_str()
        .unwrap()
        .to_string();

    // Record CompensationTerms.
    let recorded = router()
        .await
        .oneshot(record_compensation_terms_request(
            &employer_id,
            &employment_id,
            &cookie,
            true,
            json!({
                "effectiveFrom": "2026-01-01",
                "basicPayCents": 1_500_000_i64,
                "ordinaryHours": "40.00",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(recorded.status(), StatusCode::OK);

    // Both declarations.
    let declared_prior = router()
        .await
        .oneshot(declare_prior_employment_request(
            &employer_id,
            &employment_id,
            &cookie,
            true,
            2025,
        ))
        .await
        .unwrap();
    assert_eq!(declared_prior.status(), StatusCode::OK);

    let declared_unsupported = router()
        .await
        .oneshot(declare_unsupported_deductions_request(
            &employer_id,
            &employment_id,
            &cookie,
            true,
        ))
        .await
        .unwrap();
    assert_eq!(declared_unsupported.status(), StatusCode::OK);

    // Create an Ordinary run.
    let created_run = router()
        .await
        .oneshot(create_run_request(
            &employer_id,
            &cookie,
            true,
            json!({ "period": january_period(), "payDate": "2026-02-05" }),
        ))
        .await
        .unwrap();
    assert_eq!(created_run.status(), StatusCode::OK);
    let run_id = body_json(created_run).await["payrollRunId"]
        .as_str()
        .unwrap()
        .to_string();

    // Read its members.
    let detail = body_json(
        router()
            .await
            .oneshot(run_detail_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(detail["status"], "draft");
    let members = detail["members"].as_array().unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["employmentId"], employment_id);
    assert_eq!(members[0]["fullName"], "Ada Lovelace");
    // Both declarations and CompensationTerms are in force, so nothing
    // stands between this member and Calculate (§0.31).
    assert_eq!(
        members[0]["blockers"].as_array().unwrap(),
        &Vec::<Value>::new()
    );

    // Set a taxable allowance.
    let earnings_set = router()
        .await
        .oneshot(set_earnings_request(
            &employer_id,
            &run_id,
            &employment_id,
            &cookie,
            true,
            json!({
                "earnings": [{ "kind": "taxableAllowance", "amountCents": 20_000_i64, "label": "standby" }],
                "deductions": [{ "kind": "medicalAidPremium", "amountCents": 50_000_i64 }],
            }),
        ))
        .await
        .unwrap();
    assert_eq!(earnings_set.status(), StatusCode::OK);

    // Calculate.
    let calculated = router()
        .await
        .oneshot(calculate_request(&employer_id, &run_id, &cookie, true))
        .await
        .unwrap();
    assert_eq!(calculated.status(), StatusCode::OK);
    let calculated_body = body_json(calculated).await;
    assert_eq!(calculated_body["status"], "calculated");
    let member = &calculated_body["members"][0];
    assert!(member["refusal"].is_null());

    // Read the figures: §0.29's ten plus issue #78's medical aid premium,
    // all cents-exact integers and never a JSON float (INV-001).
    let figures = &member["figures"];
    let mut figure_names: Vec<&str> = figures
        .as_object()
        .expect("figures is an object")
        .keys()
        .map(String::as_str)
        .collect();
    figure_names.sort_unstable();
    assert_eq!(
        figure_names,
        [
            "basicPayCents",
            "employeeSscCents",
            "employerSscCents",
            "grossCents",
            "medicalAidPremiumCents",
            "netCents",
            "overtimeCents",
            "payeCents",
            "taxableAllowancesCents",
            "taxableRemunerationCents",
            "totalDeductionsCents",
        ],
        "the wire carries exactly §0.29's ten figures plus the medical aid premium",
    );
    let cents = |field: &str| {
        figures[field]
            .as_i64()
            .unwrap_or_else(|| panic!("{field} must be a cents-exact integer, not a JSON float"))
    };

    assert_eq!(cents("basicPayCents"), 1_500_000);
    assert_eq!(cents("taxableAllowancesCents"), 20_000);
    assert_eq!(cents("grossCents"), 1_520_000);
    assert_eq!(cents("taxableRemunerationCents"), 1_520_000);

    // The calculator really ran: both social security figures are positive.
    // Asserting only that they are integers would pass on a row of zeros.
    for field in ["employeeSscCents", "employerSscCents"] {
        assert!(cents(field) > 0, "{field} must be positive for this salary");
    }
    // PAYE is deliberately not asserted positive. This Employment starts
    // inside january_period() with no OpeningBalance, so its year-to-date
    // taxable remuneration for the whole TaxYear is this one period's
    // R15,200 — below the annual threshold, and cumulative PAYE (ADR-0001)
    // therefore owes nothing yet. Zero here is the right answer, not an
    // uncalculated one; the statutory figures themselves are proven by the
    // `payroll` crate's own ruleset tests (ADR-0008).
    assert!(cents("payeCents") >= 0);

    // The two figures that are sums are the sums of the others, and the
    // employer's own contribution is a cost to the Employer, never a
    // deduction from the employee.
    // The medical aid premium is withheld at exactly the amount sent, after
    // the two statutory deductions and never feeding either (SC-OPEN-7):
    // gross and taxable remuneration above are unchanged by it.
    assert_eq!(cents("medicalAidPremiumCents"), 50_000);
    assert_eq!(
        cents("totalDeductionsCents"),
        cents("payeCents") + cents("employeeSscCents") + cents("medicalAidPremiumCents"),
    );
    assert_eq!(
        cents("netCents"),
        cents("grossCents") - cents("totalDeductionsCents"),
    );
    assert_eq!(
        cents("grossCents"),
        cents("basicPayCents") + cents("taxableAllowancesCents"),
    );

    // Finalize.
    let finalized = router()
        .await
        .oneshot(finalize_request(&employer_id, &run_id, &cookie, true))
        .await
        .unwrap();
    assert_eq!(finalized.status(), StatusCode::OK);
    let finalized_body = body_json(finalized).await;
    let finalized_members = finalized_body["finalized"].as_array().unwrap();
    assert_eq!(finalized_members.len(), 1);
    assert_eq!(finalized_members[0]["employmentId"], employment_id);
    let finalized_payroll_id = finalized_members[0]["finalizedPayrollId"]
        .as_str()
        .unwrap()
        .to_string();

    // Read the finalized payroll back.
    let finalized_detail = body_json(
        router()
            .await
            .oneshot(finalized_detail_request(
                &employer_id,
                &finalized_payroll_id,
                &cookie,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(finalized_detail["finalizedPayrollId"], finalized_payroll_id);
    assert_eq!(finalized_detail["employmentId"], employment_id);
    assert_eq!(finalized_detail["period"], january_period());
    assert_eq!(finalized_detail["payDate"], "2026-02-05");
    assert_eq!(finalized_detail["figures"], *figures);
    assert!(!finalized_detail["saltVersion"].as_str().unwrap().is_empty());

    // Read its traces.
    let traces = body_json(
        router()
            .await
            .oneshot(finalized_traces_request(
                &employer_id,
                &finalized_payroll_id,
                &cookie,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        traces["paye"]["thisPeriodTaxableRemunerationCents"],
        1_520_000
    );
    assert!(
        !traces["paye"]["bandsApplied"]
            .as_array()
            .expect("bandsApplied is an array")
            .is_empty(),
        "a PAYE figure this size is explained by at least one band",
    );
    for role in ["employeeSsc", "employerSsc"] {
        assert_eq!(traces[role]["basicPayCents"], 1_500_000);
        assert!(
            matches!(
                traces[role]["clamp"].as_str(),
                Some("none" | "floor" | "ceiling")
            ),
            "{role}.clamp must be one of the three documented values"
        );
    }
}

// ---------------------------------------------------------------------
// Cross-Employer ids are not keys.
// ---------------------------------------------------------------------

/// Acceptance: Employer A's Operator gets 404 for Employer B's payroll run,
/// finalized payroll and Employment, each requested by its exact valid id —
/// asserted per resource kind, not once generically (§0.24: 403 would
/// confirm the id exists, a free existence oracle over payroll — ADR-0017).
#[tokio::test]
async fn employer_as_own_operator_gets_404_for_employer_bs_run_finalized_payroll_and_employment() {
    let (_a_operator, a_cookie, a_employer) = an_authorized_operator(MembershipRole::Owner).await;
    let (_b_operator, b_cookie, b_employer) = an_authorized_operator(MembershipRole::Owner).await;

    let b_employment_id = create_employment(&b_employer, &b_cookie, "Grace Hopper").await;
    fully_declare_employment(&b_employer, &b_employment_id, &b_cookie).await;
    let b_run_id = create_run(&b_employer, &b_cookie).await;
    let calculated = router()
        .await
        .oneshot(calculate_request(&b_employer, &b_run_id, &b_cookie, true))
        .await
        .unwrap();
    assert_eq!(calculated.status(), StatusCode::OK);
    let finalized = router()
        .await
        .oneshot(finalize_request(&b_employer, &b_run_id, &b_cookie, true))
        .await
        .unwrap();
    assert_eq!(finalized.status(), StatusCode::OK);
    let b_finalized_payroll_id = body_json(finalized).await["finalized"][0]["finalizedPayrollId"]
        .as_str()
        .unwrap()
        .to_string();

    // A's own URL, naming B's ids.
    let run_response = router()
        .await
        .oneshot(run_detail_request(&a_employer, &b_run_id, &a_cookie))
        .await
        .unwrap();
    assert_eq!(run_response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(run_response).await["error"]["code"],
        "payroll_run_not_found"
    );

    let finalized_response = router()
        .await
        .oneshot(finalized_detail_request(
            &a_employer,
            &b_finalized_payroll_id,
            &a_cookie,
        ))
        .await
        .unwrap();
    assert_eq!(finalized_response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(finalized_response).await["error"]["code"],
        "finalized_payroll_not_found"
    );

    let employment_response = router()
        .await
        .oneshot(employment_detail_request(
            &a_employer,
            &b_employment_id,
            &a_cookie,
        ))
        .await
        .unwrap();
    assert_eq!(employment_response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(employment_response).await["error"]["code"],
        "employment_not_found"
    );
}

// ---------------------------------------------------------------------
// No route in Spec 2 is Owner-only.
// ---------------------------------------------------------------------

/// Acceptance: a `PayrollOperator` reaches every route in Spec 2 — none of
/// them is Owner-only (§0.6). Runs the whole surface, not only the
/// tracer-bullet's own subset: both list routes, `GET /api/employers`, and
/// `opening-balance`, which the tracer bullet above does not need.
#[tokio::test]
async fn a_payroll_operator_reaches_every_route_in_spec_2() {
    let (_operator_id, cookie, employer_id) =
        an_authorized_operator(MembershipRole::PayrollOperator).await;

    assert_eq!(
        router()
            .await
            .oneshot(get_employers_request(Some(&cookie)))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    // Started well before january_period() so opening-balance has an
    // empty-covered-span to record over (mirrors tests/payroll_runs.rs's
    // own opening-balance preconditions).
    let employment_id =
        create_employment_starting(&employer_id, &cookie, "Ada Lovelace", "2025-03-01").await;

    assert_eq!(
        router()
            .await
            .oneshot(list_employments_request(&employer_id, &cookie))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        router()
            .await
            .oneshot(employment_detail_request(
                &employer_id,
                &employment_id,
                &cookie
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    assert_eq!(
        router()
            .await
            .oneshot(record_employment_end_date_request(
                &employer_id,
                &employment_id,
                &cookie,
                true
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    fully_declare_employment(&employer_id, &employment_id, &cookie).await;

    assert_eq!(
        router()
            .await
            .oneshot(record_opening_balance_request(
                &employer_id,
                &employment_id,
                &cookie,
                true
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let created_item = router()
        .await
        .oneshot(create_standing_pay_item_request(
            &employer_id,
            &employment_id,
            &cookie,
            true,
        ))
        .await
        .unwrap();
    assert_eq!(created_item.status(), StatusCode::OK);
    let standing_pay_item_id = body_json(created_item).await["standingPayItemId"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        router()
            .await
            .oneshot(list_standing_pay_items_request(
                &employer_id,
                &employment_id,
                &cookie
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let run_id = create_run(&employer_id, &cookie).await;

    // Changes the proposed line for this run only (issue #80), then catches
    // the draft up with the standing records (a no-op report here, since
    // nothing else changed), then removes the line for this run only — so
    // the empty pay-lines write below still states a complete, correct list.
    assert_eq!(
        router()
            .await
            .oneshot(override_standing_pay_line_request(
                &employer_id,
                &run_id,
                &employment_id,
                &standing_pay_item_id,
                &cookie,
                true,
                15_000,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        router()
            .await
            .oneshot(refresh_proposals_request(
                &employer_id,
                &run_id,
                &cookie,
                true
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        router()
            .await
            .oneshot(remove_standing_pay_line_request(
                &employer_id,
                &run_id,
                &employment_id,
                &standing_pay_item_id,
                &cookie,
                true,
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    // Ending the item now changes nothing about the run just proposed from
    // it: a draft holds a stated proposal (issue #79).
    assert_eq!(
        router()
            .await
            .oneshot(end_standing_pay_item_request(
                &employer_id,
                &employment_id,
                &standing_pay_item_id,
                &cookie,
                true
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    assert_eq!(
        router()
            .await
            .oneshot(list_runs_request(&employer_id, &cookie))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        router()
            .await
            .oneshot(run_detail_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        router()
            .await
            .oneshot(set_earnings_request(
                &employer_id,
                &run_id,
                &employment_id,
                &cookie,
                true,
                json!({ "earnings": [], "deductions": [] }),
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        router()
            .await
            .oneshot(calculate_request(&employer_id, &run_id, &cookie, true))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let finalized = router()
        .await
        .oneshot(finalize_request(&employer_id, &run_id, &cookie, true))
        .await
        .unwrap();
    assert_eq!(finalized.status(), StatusCode::OK);
    let finalized_payroll_id = body_json(finalized).await["finalized"][0]["finalizedPayrollId"]
        .as_str()
        .unwrap()
        .to_string();

    assert_eq!(
        router()
            .await
            .oneshot(finalized_detail_request(
                &employer_id,
                &finalized_payroll_id,
                &cookie
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        router()
            .await
            .oneshot(finalized_traces_request(
                &employer_id,
                &finalized_payroll_id,
                &cookie
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

// ---------------------------------------------------------------------
// No payroll route is reachable without a session.
// ---------------------------------------------------------------------

/// Acceptance: any payroll route called without a session returns 401.
/// Every mutating request below still carries `X-Salt-Request: 1`, so it is
/// unauthenticated status alone under test, not the header guard — and none
/// of the path ids need to name anything real: [`AuthorizedEmployerContext`]
/// (and `GET /api/employers`'s own session check) refuse before any of them
/// is ever read.
#[tokio::test]
async fn every_payroll_route_answers_401_without_a_session() {
    let e = "placeholder-employer";
    let em = "placeholder-employment";
    let p = "placeholder-person";
    let r = "placeholder-run";
    let f = "placeholder-finalized";
    let no_cookie = "";

    for request in [
        get_employers_request(None),
        get_employer_particulars_request(e, no_cookie),
        set_employer_particulars_request(
            e,
            no_cookie,
            true,
            json!({ "registeredName": "Nobody", "addressLine1": "Nowhere", "city": "Nowhere" }),
        ),
        get_person_particulars_request(e, p, no_cookie),
        set_person_particulars_request(
            e,
            p,
            no_cookie,
            true,
            json!({ "identityNumber": "1", "addressLine1": "Nowhere", "city": "Nowhere" }),
        ),
        correct_person_full_name_request(
            e,
            p,
            no_cookie,
            true,
            json!({ "fullName": "Nobody", "reason": "test" }),
        ),
        create_employment_request(e, no_cookie, true, "Nobody", "2026-01-01"),
        list_employments_request(e, no_cookie),
        employment_detail_request(e, em, no_cookie),
        record_employment_end_date_request(e, em, no_cookie, true),
        record_compensation_terms_request(
            e,
            em,
            no_cookie,
            true,
            json!({
                "effectiveFrom": "2026-01-01",
                "basicPayCents": 1,
                "ordinaryHours": "40.00",
            }),
        ),
        declare_prior_employment_request(e, em, no_cookie, true, 2025),
        declare_unsupported_deductions_request(e, em, no_cookie, true),
        record_opening_balance_request(e, em, no_cookie, true),
        list_standing_pay_items_request(e, em, no_cookie),
        create_standing_pay_item_request(e, em, no_cookie, true),
        end_standing_pay_item_request(e, em, "placeholder-item", no_cookie, true),
        create_run_request(
            e,
            no_cookie,
            true,
            json!({ "period": january_period(), "payDate": "2026-02-05" }),
        ),
        list_runs_request(e, no_cookie),
        run_detail_request(e, r, no_cookie),
        set_earnings_request(
            e,
            r,
            em,
            no_cookie,
            true,
            json!({ "earnings": [], "deductions": [] }),
        ),
        override_standing_pay_line_request(e, r, em, "placeholder-item", no_cookie, true, 1),
        remove_standing_pay_line_request(e, r, em, "placeholder-item", no_cookie, true),
        refresh_proposals_request(e, r, no_cookie, true),
        calculate_request(e, r, no_cookie, true),
        finalize_request(e, r, no_cookie, true),
        finalized_detail_request(e, f, no_cookie),
        finalized_traces_request(e, f, no_cookie),
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

// ---------------------------------------------------------------------
// Every mutating payroll route demands the CSRF header.
// ---------------------------------------------------------------------

/// Acceptance: every mutating payroll route is refused without
/// `X-Salt-Request: 1`. The header guard runs before any extractor
/// ([`salt_server::build_router`]'s own layering), so a real session and
/// real ids are not needed to prove it — a placeholder path is refused the
/// same way a real one would be.
#[tokio::test]
async fn every_mutating_payroll_route_requires_the_salt_request_header() {
    let (_operator_id, cookie, employer_id) = an_authorized_operator(MembershipRole::Owner).await;
    let em = "placeholder-employment";
    let p = "placeholder-person";
    let r = "placeholder-run";

    for request in [
        set_employer_particulars_request(
            &employer_id,
            &cookie,
            false,
            json!({ "registeredName": "Acme Corp", "addressLine1": "1 Main St", "city": "Windhoek" }),
        ),
        set_person_particulars_request(
            &employer_id,
            p,
            &cookie,
            false,
            json!({ "identityNumber": "1", "addressLine1": "1 Main St", "city": "Windhoek" }),
        ),
        correct_person_full_name_request(
            &employer_id,
            p,
            &cookie,
            false,
            json!({ "fullName": "Ada Lovelace", "reason": "test" }),
        ),
        create_employment_request(&employer_id, &cookie, false, "Nobody", "2026-01-01"),
        record_employment_end_date_request(&employer_id, em, &cookie, false),
        record_compensation_terms_request(
            &employer_id,
            em,
            &cookie,
            false,
            json!({
                "effectiveFrom": "2026-01-01",
                "basicPayCents": 1,
                "ordinaryHours": "40.00",
            }),
        ),
        declare_prior_employment_request(&employer_id, em, &cookie, false, 2025),
        declare_unsupported_deductions_request(&employer_id, em, &cookie, false),
        record_opening_balance_request(&employer_id, em, &cookie, false),
        create_standing_pay_item_request(&employer_id, em, &cookie, false),
        end_standing_pay_item_request(&employer_id, em, "placeholder-item", &cookie, false),
        create_run_request(
            &employer_id,
            &cookie,
            false,
            json!({ "period": january_period(), "payDate": "2026-02-05" }),
        ),
        set_earnings_request(
            &employer_id,
            r,
            em,
            &cookie,
            false,
            json!({ "earnings": [], "deductions": [] }),
        ),
        override_standing_pay_line_request(
            &employer_id,
            r,
            em,
            "placeholder-item",
            &cookie,
            false,
            1,
        ),
        remove_standing_pay_line_request(&employer_id, r, em, "placeholder-item", &cookie, false),
        refresh_proposals_request(&employer_id, r, &cookie, false),
        calculate_request(&employer_id, r, &cookie, false),
        finalize_request(&employer_id, r, &cookie, false),
    ] {
        let uri = request.uri().clone();
        let method = request.method().clone();
        let response = router().await.oneshot(request).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{method} {uri} must demand X-Salt-Request: 1"
        );
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "salt_request_header_required");
    }
}

// ---------------------------------------------------------------------
// A typo is never mistaken for a payroll refusal.
// ---------------------------------------------------------------------

/// Acceptance: a malformed body (400) and a payroll refusal (422) return
/// structurally different responses (§0.24) — different statuses, different
/// stable `code`s, and `details` that is always absent from a malformed
/// request but present and substantive on a refusal. Both bodies are sent to
/// the same route, `POST .../payroll-runs`, so the route itself is not the
/// variable.
#[tokio::test]
async fn a_malformed_body_and_a_payroll_refusal_are_structurally_different() {
    let (_operator_id, cookie, employer_id) = an_authorized_operator(MembershipRole::Owner).await;

    // A typo: payDate is missing entirely.
    let malformed = router()
        .await
        .oneshot(create_run_request(
            &employer_id,
            &cookie,
            true,
            json!({ "period": january_period() }),
        ))
        .await
        .unwrap();
    let malformed_status = malformed.status();
    assert_eq!(malformed_status, StatusCode::BAD_REQUEST);
    let malformed_json = body_json(malformed).await;
    assert_eq!(malformed_json["error"]["code"], "malformed_request");
    assert!(malformed_json["error"]["details"].is_null());

    // A well-formed request the payroll rules refuse: this period is not one
    // the Employer's own PaySchedule generates.
    let refused = router()
        .await
        .oneshot(create_run_request(
            &employer_id,
            &cookie,
            true,
            json!({
                "period": { "start": "2026-01-26", "end": "2026-02-25" },
                "payDate": "2026-03-05",
            }),
        ))
        .await
        .unwrap();
    let refused_status = refused.status();
    assert_eq!(refused_status, StatusCode::UNPROCESSABLE_ENTITY);
    let refused_json = body_json(refused).await;
    assert_eq!(
        refused_json["error"]["code"],
        "pay_period_not_generated_by_the_pay_schedule"
    );
    assert!(refused_json["error"]["details"].is_object());

    assert_ne!(malformed_status, refused_status);
    assert_ne!(
        malformed_json["error"]["code"],
        refused_json["error"]["code"]
    );

    // Both still speak Salt's one envelope (§0.23) — the difference is in
    // the status, the code and the details, never in the shape — and
    // neither hands a client a map of the database (§0.28, user story 42).
    for (name, json) in [("malformed", &malformed_json), ("refusal", &refused_json)] {
        let error = json["error"]
            .as_object()
            .unwrap_or_else(|| panic!("the {name} response carries an `error` object"));
        let mut keys: Vec<&str> = error.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["code", "details", "message"], "{name}");
        assert!(
            !error["message"].as_str().unwrap().is_empty(),
            "the {name} response carries a human-readable message",
        );
        let body = json.to_string().to_lowercase();
        for leak in ["select ", "insert ", "sqlx", "panicked", "backtrace"] {
            assert!(
                !body.contains(leak),
                "the {name} response must not leak {leak:?}: {body}",
            );
        }
    }
}

// ---------------------------------------------------------------------
// The three loops above are exhaustive, and stay that way.
// ---------------------------------------------------------------------

/// The `/api` route paths `crate::router` declares, read from its own
/// source at compile time. Only the literal after `.route(` is taken, so a
/// path named in a comment or a doc-comment cannot be mistaken for a
/// declared route. `/__test/*` routes are excluded: they exist only behind
/// the `test-support` feature, are no part of §0.22's contract, and none of
/// them is a payroll route these tests must walk.
fn declared_route_paths() -> Vec<String> {
    const ROUTER_SOURCE: &str = include_str!("../src/router.rs");
    let mut paths: Vec<String> = ROUTER_SOURCE
        .split(".route(")
        .skip(1)
        .filter_map(|rest| {
            let rest = rest.trim_start();
            let rest = rest.strip_prefix('"').or_else(|| {
                // `.route(\n    "…"` — the literal is on the next line.
                rest.split_once('"').map(
                    |(before, after)| {
                        if before.trim().is_empty() { after } else { "" }
                    },
                )
            })?;
            rest.split_once('"').map(|(path, _)| path.to_string())
        })
        .filter(|path| path.starts_with("/api/"))
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// The three loops in this file — 401, `PayrollOperator`, and the
/// `X-Salt-Request` header — each name every route by hand, and issue #58's
/// acceptance criteria say "every". A route added to `build_router` would
/// otherwise escape all three in silence. This test fails the moment the
/// declared route table stops being exactly §0.22's own, so adding a route
/// forces a decision about those loops rather than allowing an omission.
/// It is also parent #49's user story 48: the route table is exactly §0.22
/// and nothing more.
#[test]
fn the_declared_route_table_is_exactly_the_one_these_tests_walk() {
    assert_eq!(
        declared_route_paths(),
        [
            "/api/employers",
            "/api/employers/{employer_id}/employments",
            "/api/employers/{employer_id}/employments/{employment_id}",
            "/api/employers/{employer_id}/employments/{employment_id}/compensation-terms",
            "/api/employers/{employer_id}/employments/{employment_id}/end-date",
            "/api/employers/{employer_id}/employments/{employment_id}/opening-balance",
            "/api/employers/{employer_id}/employments/{employment_id}/prior-employment",
            "/api/employers/{employer_id}/employments/{employment_id}/standing-pay-items",
            "/api/employers/{employer_id}/employments/{employment_id}/standing-pay-items/{standing_pay_item_id}/end",
            "/api/employers/{employer_id}/employments/{employment_id}/unsupported-deductions",
            "/api/employers/{employer_id}/finalized-payroll/{finalized_payroll_id}",
            "/api/employers/{employer_id}/finalized-payroll/{finalized_payroll_id}/traces",
            "/api/employers/{employer_id}/particulars",
            "/api/employers/{employer_id}/payroll-runs",
            "/api/employers/{employer_id}/payroll-runs/{payroll_run_id}",
            "/api/employers/{employer_id}/payroll-runs/{payroll_run_id}/calculate",
            "/api/employers/{employer_id}/payroll-runs/{payroll_run_id}/finalize",
            "/api/employers/{employer_id}/payroll-runs/{payroll_run_id}/members/{employment_id}/pay-lines",
            "/api/employers/{employer_id}/payroll-runs/{payroll_run_id}/members/{employment_id}/pay-lines/{standing_pay_item_id}/override",
            "/api/employers/{employer_id}/payroll-runs/{payroll_run_id}/members/{employment_id}/pay-lines/{standing_pay_item_id}/remove",
            "/api/employers/{employer_id}/payroll-runs/{payroll_run_id}/refresh-proposals",
            "/api/employers/{employer_id}/people/{person_id}/name",
            "/api/employers/{employer_id}/people/{person_id}/particulars",
            "/api/health",
            "/api/ready",
            "/api/session",
        ],
        "the route table changed: revisit every `for request in [..]` loop in \
         this file before changing this list",
    );
}

/// Guards the guard: a parser that found nothing would pass the test above
/// for the wrong reason, and one that swept up comments would pass it for a
/// different wrong reason.
#[test]
fn the_router_source_parser_finds_the_routes_that_are_there() {
    let paths = declared_route_paths();
    assert_eq!(paths.len(), 26, "{paths:?}");
    assert!(paths.iter().any(|path| path == "/api/health"), "{paths:?}");
    assert!(
        paths
            .iter()
            .any(|path| path == "/api/employers/{employer_id}/payroll-runs"),
        "{paths:?}"
    );
}
