//! Proves `POST /api/employers/{e}/payroll-runs`, `GET
//! /api/employers/{e}/payroll-runs`, `GET
//! /api/employers/{e}/payroll-runs/{r}` and `PUT
//! /api/employers/{e}/payroll-runs/{r}/members/{em}/pay-lines` (issue #53,
//! parent #49 Spec 2 of 3; renamed from `.../earnings` by issue #77), plus `POST
//! /api/employers/{e}/payroll-runs/{r}/calculate` (issue #55) and `POST
//! /api/employers/{e}/payroll-runs/{r}/finalize` (issue #56). Driven with
//! `tower::ServiceExt::oneshot` against the real router, the same
//! discipline `tests/employments.rs` already follows.

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use payroll_app::{
    DatabaseConfig, MembershipRole, OperatorId, SaltDatabase, StandingPayItemInstruction,
};
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

fn create_employment_request(
    employer_id: &str,
    cookie: &str,
    full_name: &str,
    start_date: &str,
) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/api/employers/{employer_id}/employments"))
        .header(header::COOKIE, cookie)
        .header("x-salt-request", "1")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({ "fullName": full_name, "startDate": start_date }).to_string(),
        ))
        .unwrap()
}

async fn create_employment(employer_id: &str, cookie: &str, full_name: &str) -> String {
    create_employment_starting(employer_id, cookie, full_name, "2026-01-01").await
}

/// An Employment whose `startDate` is chosen by the caller. Only the one
/// test that adopts Salt part-way through a TaxYear needs a start earlier
/// than `january_period()`; every other test wants the default above.
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
                "ordinaryHours": "40.00",
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

/// `POST /api/employers/{e}/employments/{em}/opening-balance`, the route
/// issue #52 already ships. Used here to record — and then to change — the
/// one fact an Operator can still move after a run is `Calculated` without
/// the run itself reopening.
fn record_opening_balance_request(
    employer_id: &str,
    employment_id: &str,
    cookie: &str,
    salt_coverage_start: &str,
    prior_taxable_remuneration_cents: i64,
    prior_paye_cents: i64,
) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!(
            "/api/employers/{employer_id}/employments/{employment_id}/opening-balance"
        ))
        .header(header::COOKIE, cookie)
        .header("x-salt-request", "1")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({
                "taxYear": 2025,
                "saltCoverageStart": salt_coverage_start,
                "priorTaxableRemunerationCents": prior_taxable_remuneration_cents,
                "priorPayeCents": prior_paye_cents,
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
                "deductions": [],
                "earnings": [
                    { "kind": "taxableAllowance", "amountCents": 5000, "label": "  standby  " },
                    { "kind": "taxableAllowance", "amountCents": 2500, "label": "travel" },
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
    assert_eq!(earnings[0]["label"], "standby");
    assert_eq!(earnings[1]["label"], "travel");

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
                "deductions": [],
                "earnings": [{ "kind": "taxableAllowance", "amountCents": 1000, "label": "travel" }],
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
    assert_eq!(earnings[0]["label"], "travel");
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
            serde_json::json!({ "earnings": [], "deductions": [] }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "salt_request_header_required");
}

/// `calculate` derives `BasicPay` itself from `CompensationTerms`, so a
/// `BasicPay` line is not part of the earning-instruction input shape.
#[tokio::test]
async fn a_basic_pay_line_is_a_malformed_request() {
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
                "deductions": [],
                "earnings": [{ "kind": "basicPay", "amountCents": 150000 }],
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "malformed_request");
}

// Issue #76: overtime is typed as hours at a multiplier, never as money,
// and reads back the same way. `amountCents` is deliberately absent from
// both the request and the response — Salt derives the money.
#[tokio::test]
async fn overtime_is_set_and_read_back_as_hours_at_a_multiplier() {
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
                "deductions": [],
                "earnings": [
                    { "kind": "overtime", "hours": "12", "multiplier": "1.5",
                      "label": "  Sunday overtime  " },
                    { "kind": "overtime", "hours": "4.5", "multiplier": "2",
                      "label": "public holiday" },
                    { "kind": "overtime", "hours": "2", "multiplier": "1.5" },
                    { "kind": "overtime", "hours": "1", "multiplier": "2",
                      "label": "   " },
                ],
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let detail = body_json(
        router()
            .await
            .oneshot(detail_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    let earnings = detail["members"][0]["earnings"].as_array().unwrap();
    assert_eq!(earnings.len(), 4);
    assert_eq!(earnings[0]["kind"], "overtime");
    assert_eq!(earnings[0]["hours"], "12");
    assert_eq!(earnings[0]["multiplier"], "1.5");
    assert_eq!(earnings[0]["label"], "Sunday overtime");
    assert!(
        earnings[0].get("amountCents").is_none(),
        "an overtime instruction carries hours, never money"
    );
    assert_eq!(earnings[1]["hours"], "4.5");
    assert_eq!(earnings[1]["multiplier"], "2");
    assert_eq!(earnings[2]["label"], serde_json::Value::Null);
    assert_eq!(earnings[3]["label"], serde_json::Value::Null);
}

// D31: the multiplier set is closed at 1.5 and 2.0. The body is well
// formed, so this is not `malformed_request` — it is its own refusal with
// its own stated reason and the supported set named.
#[tokio::test]
async fn an_overtime_multiplier_outside_the_closed_set_is_refused_with_a_stated_reason() {
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
                "deductions": [],
                "earnings": [{ "kind": "overtime", "hours": "12", "multiplier": "1.75",
                               "label": "overtime" }],
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "unsupported_overtime_multiplier");
    assert_eq!(json["error"]["details"]["supplied"], "1.75");
    assert_eq!(
        json["error"]["details"]["supported"],
        serde_json::json!(["1.5", "2"])
    );
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("1.5 and 2"),
        "the refusal must state which multipliers are supported"
    );
}

#[tokio::test]
async fn zero_or_negative_overtime_hours_are_refused() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let run_id = create_run(&employer_id, &cookie).await;

    for hours in ["0", "-3"] {
        let response = router()
            .await
            .oneshot(set_earnings_request(
                &employer_id,
                &run_id,
                &employment_id,
                &cookie,
                true,
                serde_json::json!({
                    "deductions": [],
                    "earnings": [{ "kind": "overtime", "hours": hours, "multiplier": "1.5",
                                   "label": "overtime" }],
                }),
            ))
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "hours {hours}"
        );
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "invalid_overtime_hours");
        assert_eq!(json["error"]["details"]["supplied"], hours);
    }
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
            serde_json::json!({ "earnings": [], "deductions": [] }),
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
            serde_json::json!({ "earnings": [], "deductions": [] }),
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
            serde_json::json!({ "earnings": [], "deductions": [] }),
        ),
        calculate_request(&employer_id, &run_id, &outsider_cookie, true),
        finalize_request(&employer_id, &run_id, &outsider_cookie, true),
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
            serde_json::json!({ "earnings": [], "deductions": [] }),
        ),
        calculate_request(&employer_id, &run_id, no_cookie, true),
        finalize_request(&employer_id, &run_id, no_cookie, true),
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
        serde_json::json!({ "deductions": [], "earnings": [{ "kind": "bonus", "amountCents": 1000 }] }),
        serde_json::json!({ "deductions": [], "earnings": [{ "kind": "taxableAllowance", "amountCents": -1 }] }),
        serde_json::json!({ "deductions": [], "earnings": [{ "kind": "taxableAllowance" }] }),
        serde_json::json!({ "deductions": [], "earnings": [{ "kind": "taxableAllowance", "amountCents": 1000 }] }),
        serde_json::json!({ "deductions": [], "earnings": [{ "kind": "taxableAllowance", "amountCents": 1000, "label": "  " }] }),
        serde_json::json!({ "deductions": [], "earnings": [{ "kind": "taxableAllowance", "amountCents": 1000, "label": "x".repeat(101) }] }),
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
                "deductions": [],
                "earnings": [{ "kind": "taxableAllowance", "amountCents": 5000, "label": "standby" }],
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
                "deductions": [],
                "earnings": [
                    { "kind": "taxableAllowance", "amountCents": 1000, "label": "travel" },
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
                "deductions": [],
                "earnings": [{ "kind": "taxableAllowance", "amountCents": 5000, "label": "standby" }],
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
            serde_json::json!({ "earnings": [], "deductions": [] }),
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
    fully_declare_employment(&employer_id, &employment_id, &cookie).await;
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
            serde_json::json!({ "earnings": [], "deductions": [] }),
        ),
        calculate_request(&employer_id, &run_id, &cookie, true),
        finalize_request(&employer_id, &run_id, &cookie, true),
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
    assert_eq!(refreshed["members"][0]["calculationState"], "current");
    assert!(refreshed["members"][0]["refusal"].is_null());

    // A pay-line write removes the old calculation in the same transaction
    // (issue #77). The later plain GET names that absence as a save, rather
    // than making the worksheet guess whether this member was never
    // calculated at all.
    let response = router()
        .await
        .oneshot(set_earnings_request(
            &employer_id,
            &run_id,
            &employment_id,
            &cookie,
            true,
            serde_json::json!({
                "deductions": [],
                "earnings": [{ "kind": "taxableAllowance", "amountCents": 5000, "label": "standby" }],
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let after_edit = body_json(
        router()
            .await
            .oneshot(detail_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(after_edit["status"], "draft");
    assert!(after_edit["members"][0]["figures"].is_null());
    assert_eq!(
        after_edit["members"][0]["calculationState"],
        "pay_lines_saved"
    );

    // The next Calculate makes the figures current again, from the new line.
    let recalculated = body_json(
        router()
            .await
            .oneshot(calculate_request(&employer_id, &run_id, &cookie, true))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(recalculated["members"][0]["calculationState"], "current");
    assert_eq!(
        recalculated["members"][0]["figures"]["taxableAllowancesCents"],
        5000
    );
}

/// `PUT .../pay-lines` replaced `PUT .../earnings` (issue #77); the old
/// route is removed, not left standing beside the new one where a caller
/// could still wipe provenance. It answers exactly like any path that does
/// not exist, and writes nothing.
#[tokio::test]
async fn the_old_earnings_route_is_gone() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let run_id = create_run(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!(
                    "/api/employers/{employer_id}/payroll-runs/{run_id}/members/{employment_id}/earnings"
                ))
                .header(header::COOKIE, &cookie)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-salt-request", "1")
                .body(Body::from(
                    serde_json::json!({
                        "deductions": [],
                        "earnings": [
                            { "kind": "taxableAllowance", "amountCents": 5000, "label": "standby" },
                        ],
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let detail = body_json(
        router()
            .await
            .oneshot(detail_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(detail["members"][0]["earnings"], serde_json::json!([]));
}

/// Provenance is Salt's record, not the caller's claim (issue #77): every
/// line typed through the route reads back `one_off`, even when the body
/// asserts otherwise, and writing lines for a member who was never
/// calculated does not claim stale figures that never existed.
#[tokio::test]
async fn a_written_line_reads_back_one_off_whatever_source_the_body_claims() {
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
                "deductions": [],
                "earnings": [
                    {
                        "kind": "taxableAllowance",
                        "amountCents": 5000,
                        "label": "standby",
                        "source": "from_reversed_snapshot",
                    },
                    { "kind": "overtime", "hours": "4", "multiplier": "1.5", "label": null },
                ],
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let detail = body_json(
        router()
            .await
            .oneshot(detail_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    let member = &detail["members"][0];
    assert_eq!(member["earnings"][0]["source"], "one_off");
    assert_eq!(member["earnings"][1]["source"], "one_off");
    assert!(member["figures"].is_null());
    assert_eq!(member["calculationState"], "not_calculated");
}

/// Issue #79's proposal is readable as both a standing line and the date it
/// began; an id alone would leave the worksheet unable to say "since when".
#[tokio::test]
async fn a_standing_pay_line_reads_back_with_its_effective_from_date() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let db = test_db().await;
    let item_id = payroll_app::create_standing_pay_item(
        &db,
        &payroll::EmploymentId::new(employment_id.clone()),
        StandingPayItemInstruction::TaxableAllowance {
            amount: payroll::Money::from_cents(5_000).unwrap(),
            label: Some(payroll::EarningLabel::new("standby").unwrap()),
        },
        chrono::NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
        "test-setup",
    )
    .await
    .unwrap();
    let run_id = create_run(&employer_id, &cookie).await;

    let detail = body_json(
        router()
            .await
            .oneshot(detail_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;

    assert_eq!(
        detail["members"][0]["earnings"],
        serde_json::json!([{
            "kind": "taxableAllowance",
            "amountCents": 5_000,
            "label": "standby",
            "source": "standing",
            "standingPayItemId": item_id.to_string(),
            "standingEffectiveFrom": "2026-01-01",
        }])
    );
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

    let finalized = router()
        .await
        .oneshot(finalize_request(&employer_id, &run_id, &cookie, true))
        .await
        .unwrap();
    assert_eq!(finalized.status(), StatusCode::OK);

    let response = router()
        .await
        .oneshot(calculate_request(&employer_id, &run_id, &cookie, true))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "payroll_run_already_finalized");
}

/// "A run in which **every** member refused still returns 200" (issue
/// #55's own acceptance criterion), proved with **two** members rather than
/// one: a single-member run cannot tell "the whole run refused" apart from
/// "the only member refused", and it is the plural case the criterion is
/// about.
#[tokio::test]
async fn a_run_in_which_every_member_refused_is_still_200() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let first_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let second_id = create_employment(&employer_id, &cookie, "Grace Hopper").await;
    let run_id = create_run(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(calculate_request(&employer_id, &run_id, &cookie, true))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "every member refusing is a normal outcome, not an error (§0.25)"
    );
    let detail = body_json(response).await;
    assert_eq!(
        detail["status"], "draft",
        "no member calculated, so Finalize is not allowed"
    );
    let members = detail["members"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    for employment_id in [&first_id, &second_id] {
        let member = members
            .iter()
            .find(|member| member["employmentId"] == *employment_id)
            .expect("every member of the run is still listed");
        assert!(member["figures"].is_null());
        assert_eq!(member["refusal"]["code"], "no_compensation_terms_in_force");
    }
}

/// A per-member `refusal` carries **details**, not only a code (issue #55's
/// own acceptance criterion), and a `blocker` and a `refusal` are different
/// things that may both be present on one member (its own Deep
/// Instructions). A member declared to have unsupported deductions is the
/// case that shows both at once: `blockers` names the standing fact,
/// `refusal` is what the calculator actually said about it, and each
/// carries the kinds a screen must name.
#[tokio::test]
async fn a_member_refusal_carries_its_details_beside_its_own_blocker() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;

    let recorded = router()
        .await
        .oneshot(record_compensation_terms_request(
            &employer_id,
            &employment_id,
            &cookie,
            "2026-01-01",
            1_500_000,
        ))
        .await
        .unwrap();
    assert_eq!(recorded.status(), StatusCode::OK);

    let declared = router()
        .await
        .oneshot(declare_prior_employment_request(
            &employer_id,
            &employment_id,
            &cookie,
            2025,
        ))
        .await
        .unwrap();
    assert_eq!(declared.status(), StatusCode::OK);

    let declared = router()
        .await
        .oneshot(declare_unsupported_deductions_request(
            &employer_id,
            &employment_id,
            &cookie,
            serde_json::json!({
                "effectiveFrom": "2026-01-01",
                "status": "present",
                "kinds": ["provident_fund"],
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
        .oneshot(calculate_request(&employer_id, &run_id, &cookie, true))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let detail = body_json(response).await;
    let member = &detail["members"][0];
    assert!(member["figures"].is_null());
    assert_eq!(member["refusal"]["code"], "unsupported_deductions_present");
    assert_eq!(
        member["refusal"]["details"]["kinds"],
        serde_json::json!(["provident_fund"]),
        "a refusal that cannot be acted on without naming the kinds must name them"
    );
    assert!(
        member["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|blocker| blocker["code"] == "unsupported_deductions_present"),
        "a blocker read from standing facts and a refusal from the calculator \
         are different fields, and both are present here"
    );
}

/// A member whose calculation succeeded and then refused on a later
/// Calculate must not keep showing the figures from the earlier one: stale
/// figures beside a fresh refusal is the one way this response could lie
/// about money.
#[tokio::test]
async fn recalculating_after_a_fact_is_withdrawn_clears_the_stale_figures() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    fully_declare_employment(&employer_id, &employment_id, &cookie).await;
    let run_id = create_run(&employer_id, &cookie).await;

    let calculated = body_json(
        router()
            .await
            .oneshot(calculate_request(&employer_id, &run_id, &cookie, true))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(calculated["status"], "calculated");
    assert_eq!(
        calculated["members"][0]["figures"]["basicPayCents"],
        1_500_000
    );

    // The same period now has unsupported deductions declared present, so
    // the next Calculate refuses the member it previously calculated.
    let declared = router()
        .await
        .oneshot(declare_unsupported_deductions_request(
            &employer_id,
            &employment_id,
            &cookie,
            serde_json::json!({
                "effectiveFrom": "2026-01-01",
                "status": "present",
                "kinds": ["provident_fund"],
                "acknowledgedDivergingPeriods": [],
                "reason": "joined a provident fund",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(declared.status(), StatusCode::OK);

    let recalculated = body_json(
        router()
            .await
            .oneshot(calculate_request(&employer_id, &run_id, &cookie, true))
            .await
            .unwrap(),
    )
    .await;

    assert_eq!(recalculated["status"], "draft");
    assert!(
        recalculated["members"][0]["figures"].is_null(),
        "a refused member never keeps the figures of an earlier calculation"
    );
    assert_eq!(
        recalculated["members"][0]["refusal"]["code"],
        "unsupported_deductions_present"
    );

    // And a plain refresh agrees: the stored calculation is gone, not just
    // hidden by Calculate's own response.
    let refreshed = body_json(
        router()
            .await
            .oneshot(detail_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(refreshed["status"], "draft");
    assert!(refreshed["members"][0]["figures"].is_null());
}

// ---- issue #56: `POST /api/employers/{e}/payroll-runs/{r}/finalize` ----

/// The route's own happy path: a `Calculated` run's active member finalizes
/// into immutable history, and the response carries the id an Operator needs
/// to go and open what was just written — its own `employmentId` beside the
/// new `finalizedPayrollId` — while a later refresh shows both the run as
/// `"finalized"` and that member's immutable record.
#[tokio::test]
async fn finalizing_a_calculated_run_returns_each_members_finalized_payroll_id() {
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

    let response = router()
        .await
        .oneshot(finalize_request(&employer_id, &run_id, &cookie, true))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    let finalized = json["finalized"].as_array().unwrap();
    assert_eq!(finalized.len(), 1);
    assert_eq!(finalized[0]["employmentId"], employment_id);
    assert!(
        !finalized[0]["finalizedPayrollId"]
            .as_str()
            .unwrap()
            .is_empty()
    );

    let refreshed = body_json(
        router()
            .await
            .oneshot(detail_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(refreshed["status"], "finalized");
    assert_eq!(
        refreshed["members"][0]["finalizedPayrollId"],
        json["finalized"][0]["finalizedPayrollId"]
    );
}

#[tokio::test]
async fn the_finalize_route_requires_the_salt_request_header() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let run_id = create_run(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(finalize_request(&employer_id, &run_id, &cookie, false))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "salt_request_header_required");
}

#[tokio::test]
async fn a_run_id_belonging_to_another_employer_is_not_found_on_the_finalize_route() {
    let (owning_cookie, owning_employer) = an_authorized_operator().await;
    let (other_cookie, other_employer) = an_authorized_operator().await;
    let run_id = create_run(&owning_employer, &owning_cookie).await;

    let response = router()
        .await
        .oneshot(finalize_request(
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

/// A run that was never calculated — still `Draft` — is refused with its own
/// stable code, distinct from the already-finalized 409 below.
#[tokio::test]
async fn finalizing_a_run_that_was_never_calculated_is_refused_with_its_own_stable_code() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let run_id = create_run(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(finalize_request(&employer_id, &run_id, &cookie, true))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "payroll_run_not_calculated");
}

/// A retried Finalize against an already-`Finalized` run is refused with
/// `payroll_run_already_finalized` and `details.finalizedPayrollId` names the
/// payroll that already exists — the recovery *is* the 409 body (§0.28): a
/// lost response is answered with the success that already happened, not an
/// unexplained error, and there is no idempotency key mechanism behind it.
#[tokio::test]
async fn retrying_finalize_on_an_already_finalized_run_answers_409_with_the_finalized_payroll_id() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    fully_declare_employment(&employer_id, &employment_id, &cookie).await;
    let run_id = create_run(&employer_id, &cookie).await;

    router()
        .await
        .oneshot(calculate_request(&employer_id, &run_id, &cookie, true))
        .await
        .unwrap();
    let first = body_json(
        router()
            .await
            .oneshot(finalize_request(&employer_id, &run_id, &cookie, true))
            .await
            .unwrap(),
    )
    .await;
    let finalized_payroll_id = first["finalized"][0]["finalizedPayrollId"]
        .as_str()
        .unwrap()
        .to_string();

    let retried = router()
        .await
        .oneshot(finalize_request(&employer_id, &run_id, &cookie, true))
        .await
        .unwrap();

    assert_eq!(retried.status(), StatusCode::CONFLICT);
    let json = body_json(retried).await;
    assert_eq!(json["error"]["code"], "payroll_run_already_finalized");
    assert_eq!(
        json["error"]["details"]["finalizedPayrollId"],
        finalized_payroll_id
    );
}

/// A fact that moved since Calculate without a recalculation in between
/// (issue #56's own acceptance criterion) — proved here with
/// `UnsupportedDeductionStatus` turning `present`, which `finalize`'s rebuild
/// of the member's `PayrollInput` re-reads and refuses on, unlike
/// `payroll_run.status`, which stays `"calculated"` because declaring a
/// standing fact is Employment-scoped and never reopens a run the way
/// `set_run_pay_lines` and a membership removal do.
///
/// This lands as `finalization_rebuild_refused`, not one of the three named
/// mismatch codes: those three name a rebuild that *succeeds* but disagrees
/// with what was approved, and a fact that blocks the rebuild outright never
/// gets that far. The named codes have their own test below
/// (`finalizing_after_the_opening_balance_moved_answers_finalization_input_mismatch`);
/// the two refusals are different answers to "a fact moved", and both are a
/// 409 an Operator recovers from by calculating again.
#[tokio::test]
async fn finalizing_after_a_fact_moved_without_recalculating_is_refused() {
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

    let declared = router()
        .await
        .oneshot(declare_unsupported_deductions_request(
            &employer_id,
            &employment_id,
            &cookie,
            serde_json::json!({
                "effectiveFrom": "2026-01-01",
                "status": "present",
                "kinds": ["provident_fund"],
                "acknowledgedDivergingPeriods": [],
                "reason": "joined a provident fund",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(declared.status(), StatusCode::OK);

    let response = router()
        .await
        .oneshot(finalize_request(&employer_id, &run_id, &cookie, true))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "finalization_rebuild_refused");
    assert_eq!(json["error"]["details"]["employmentId"], employment_id);
    assert_eq!(
        json["error"]["details"]["refusalCode"],
        "unsupported_deductions_present"
    );
}

/// Issue #56's mismatch criterion, reached through the routes this spec
/// actually ships: an `OpeningBalance` is the one fact an Operator can still
/// move after Calculate that the rebuild *re-reads successfully* and then
/// disagrees about. Its figures land in the member's `YearToDateContext`,
/// which is part of the frozen `PayrollInput`, so a changed balance makes
/// the reassembled input differ from the approved one and finalization is
/// refused with `finalization_input_mismatch` naming the employment — the
/// first of §5.2's three comparisons, which stop at the first that differs.
///
/// The Employment starts in March 2025 and the balance says Salt's coverage
/// begins with January 2026, so December 2025 is resolved by the balance
/// itself (§7.1 branch 2) and the Ordinary run's own sequencing check passes.
#[tokio::test]
async fn finalizing_after_the_opening_balance_moved_answers_finalization_input_mismatch() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id =
        create_employment_starting(&employer_id, &cookie, "Grace Hopper", "2025-03-01").await;
    fully_declare_employment(&employer_id, &employment_id, &cookie).await;

    let recorded = router()
        .await
        .oneshot(record_opening_balance_request(
            &employer_id,
            &employment_id,
            &cookie,
            "2026-01-31",
            3_000_000,
            0,
        ))
        .await
        .unwrap();
    assert_eq!(recorded.status(), StatusCode::OK);

    let run_id = create_run(&employer_id, &cookie).await;
    let calculated = router()
        .await
        .oneshot(calculate_request(&employer_id, &run_id, &cookie, true))
        .await
        .unwrap();
    assert_eq!(calculated.status(), StatusCode::OK);

    // The same balance, corrected: the year-to-date position the Operator
    // approved is no longer the one Salt now holds.
    let corrected = router()
        .await
        .oneshot(record_opening_balance_request(
            &employer_id,
            &employment_id,
            &cookie,
            "2026-01-31",
            6_000_000,
            0,
        ))
        .await
        .unwrap();
    assert_eq!(corrected.status(), StatusCode::OK);

    let response = router()
        .await
        .oneshot(finalize_request(&employer_id, &run_id, &cookie, true))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "finalization_input_mismatch");
    assert_eq!(json["error"]["details"]["employmentId"], employment_id);

    // A refused finalization writes no history at all (§5.1), so the run is
    // still the `Calculated` one the Operator can recalculate.
    let refreshed = body_json(
        router()
            .await
            .oneshot(detail_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(refreshed["status"], "calculated");
}

/// Two finalizers of one run, in flight together: the run's own `FOR UPDATE`
/// lock serialises them (§5.4), so exactly one `FinalizedPayroll` is ever
/// written and the loser reads back the winner's own id under
/// `payroll_run_already_finalized` — the same recovery a retry after a lost
/// response gets, because it is the same situation. Each request goes
/// through its own router, and therefore its own connection pool, so the
/// two really do contend in the database rather than queueing on one
/// connection.
#[tokio::test]
async fn two_concurrent_finalizes_write_exactly_one_finalized_payroll() {
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

    let first_router = router().await;
    let second_router = router().await;
    let (first, second) = tokio::join!(
        first_router.oneshot(finalize_request(&employer_id, &run_id, &cookie, true)),
        second_router.oneshot(finalize_request(&employer_id, &run_id, &cookie, true)),
    );
    let (first, second) = (first.unwrap(), second.unwrap());

    let (winner, loser) = if first.status() == StatusCode::OK {
        (first, second)
    } else {
        (second, first)
    };
    assert_eq!(winner.status(), StatusCode::OK);
    assert_eq!(loser.status(), StatusCode::CONFLICT);

    let winner = body_json(winner).await;
    let finalized = winner["finalized"].as_array().unwrap();
    assert_eq!(finalized.len(), 1);
    let finalized_payroll_id = finalized[0]["finalizedPayrollId"].as_str().unwrap();

    let loser = body_json(loser).await;
    assert_eq!(loser["error"]["code"], "payroll_run_already_finalized");
    assert_eq!(
        loser["error"]["details"]["finalizedPayrollId"],
        finalized_payroll_id
    );
}

// ---- Medical aid premium deductions (issue #78) ----

/// Deductions travel in their own `deductions` array and read back in the
/// run detail beside the earnings, each with the source Salt recorded.
#[tokio::test]
async fn deductions_are_set_and_read_back_with_their_source() {
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
                "earnings": [{ "kind": "taxableAllowance", "amountCents": 5000, "label": "standby" }],
                "deductions": [{ "kind": "medicalAidPremium", "amountCents": 75000, "source": "from_reversed_snapshot" }],
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let detail = body_json(
        router()
            .await
            .oneshot(detail_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    let member = &detail["members"][0];
    assert_eq!(member["earnings"].as_array().unwrap().len(), 1);
    assert_eq!(
        member["deductions"],
        serde_json::json!([{ "kind": "medicalAidPremium", "amountCents": 75000, "source": "one_off" }]),
        "a claimed source is ignored: provenance is Salt's record"
    );
}

/// §D-4: a zero-amount deduction is not a line, refused under its own code
/// naming which line, and nothing is written.
#[tokio::test]
async fn a_zero_amount_deduction_is_refused_with_its_own_code() {
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
                "earnings": [],
                "deductions": [
                    { "kind": "medicalAidPremium", "amountCents": 75000 },
                    { "kind": "medicalAidPremium", "amountCents": 0 },
                ],
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "voluntary_deduction_amount_is_zero");
    assert_eq!(json["error"]["details"]["index"], 1);

    let detail = body_json(
        router()
            .await
            .oneshot(detail_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(detail["members"][0]["deductions"], serde_json::json!([]));
}

/// Exactly one deduction kind exists, and a body is a complete statement:
/// an unknown kind, a negative or missing amount, or an absent `deductions`
/// array is malformed — never read as "no deductions", which would silently
/// delete the ones stored.
#[tokio::test]
async fn a_deduction_salt_cannot_represent_is_a_bad_request() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let run_id = create_run(&employer_id, &cookie).await;

    for body in [
        serde_json::json!({ "earnings": [], "deductions": [{ "kind": "pension", "amountCents": 1000 }] }),
        serde_json::json!({ "earnings": [], "deductions": [{ "kind": "other", "amountCents": 1000, "label": "gym" }] }),
        serde_json::json!({ "earnings": [], "deductions": [{ "kind": "medicalAidPremium", "amountCents": -1 }] }),
        serde_json::json!({ "earnings": [], "deductions": [{ "kind": "medicalAidPremium" }] }),
        serde_json::json!({ "earnings": [] }),
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
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
    }
}

/// Q-OPEN-22 over the wire: a premium above the net pay left after PAYE and
/// social security is refused on Calculate, naming the exact shortfall in
/// cents, and the member has no figures.
#[tokio::test]
async fn a_premium_above_available_net_pay_is_refused_naming_the_shortfall() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    fully_declare_employment(&employer_id, &employment_id, &cookie).await;
    let run_id = create_run(&employer_id, &cookie).await;

    let calculated = body_json(
        router()
            .await
            .oneshot(calculate_request(&employer_id, &run_id, &cookie, true))
            .await
            .unwrap(),
    )
    .await;
    let available = calculated["members"][0]["figures"]["netCents"]
        .as_i64()
        .unwrap();

    let response = router()
        .await
        .oneshot(set_earnings_request(
            &employer_id,
            &run_id,
            &employment_id,
            &cookie,
            true,
            serde_json::json!({
                "earnings": [],
                "deductions": [{ "kind": "medicalAidPremium", "amountCents": available + 1234 }],
            }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let refused = body_json(
        router()
            .await
            .oneshot(calculate_request(&employer_id, &run_id, &cookie, true))
            .await
            .unwrap(),
    )
    .await;
    let member = &refused["members"][0];
    assert!(member["figures"].is_null());
    assert_eq!(
        member["refusal"],
        serde_json::json!({
            "code": "deductions_exceed_gross_remuneration",
            "details": { "shortfallCents": 1234 },
        })
    );
}

/// Employer-paid medical aid is declarable by its own code and blocks the
/// member by name, before any pay line is set up.
#[tokio::test]
async fn employer_paid_medical_aid_is_declarable_and_blocks_by_name_on_the_wire() {
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
                "kinds": ["employer_paid_medical_aid"],
                "acknowledgedDivergingPeriods": [],
                "reason": "employer pays the medical aid",
            }),
        ))
        .await
        .unwrap();
    assert_eq!(declared.status(), StatusCode::OK);

    let run_id = create_run(&employer_id, &cookie).await;
    let detail = body_json(
        router()
            .await
            .oneshot(detail_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    let present = detail["members"][0]["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|blocker| blocker["code"] == "unsupported_deductions_present")
        .expect("employer-paid medical aid blocks under the present-kinds code")
        .clone();
    assert_eq!(
        present["details"]["kinds"],
        serde_json::json!(["employer_paid_medical_aid"])
    );
}
