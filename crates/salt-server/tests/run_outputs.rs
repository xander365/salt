//! Proves `GET /api/employers/{e}/payroll-runs/{r}/register` and `GET
//! /api/employers/{e}/payroll-runs/{r}/payment-summary` (issue #83, parent
//! #70 §D-9, §D-10). Driven with `tower::ServiceExt::oneshot` against the
//! real router, the same discipline `tests/payslip.rs` already follows;
//! fixtures below are copied from that file rather than shared, matching
//! this codebase's existing per-file fixture discipline.

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::NaiveDate;
use payroll_app::{
    DatabaseConfig, EmployerParticularsFields, EmploymentPerson, MembershipRole, OperatorId,
    SaltDatabase,
};
use salt_server::{AppState, build_router, rendered_text};
use serde_json::Value;
use tower::ServiceExt;

fn test_database_config() -> DatabaseConfig {
    let url = std::env::var("DATABASE_URL").expect(
        "DATABASE_URL must name an already-migrated database for salt-server's own tests \
         (see crates/salt-server/tests/run_outputs.rs)",
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
    session_cookie_pair(&response)
}

fn session_cookie_pair(response: &axum::response::Response) -> String {
    let set_cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .expect("a login response always carries Set-Cookie")
        .to_str()
        .unwrap();
    set_cookie
        .split(';')
        .next()
        .expect("Set-Cookie always carries at least the name=value pair")
        .to_string()
}

async fn an_authorized_operator() -> (String, String, String) {
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
    (email, cookie, employer_id.to_string())
}

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

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

async fn create_run(employer_id: &str, cookie: &str) -> String {
    let response = router()
        .await
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/employers/{employer_id}/payroll-runs"))
                .header(header::COOKIE, cookie)
                .header("x-salt-request", "1")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({ "period": january_period(), "payDate": "2026-02-05" })
                        .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await["payrollRunId"]
        .as_str()
        .unwrap()
        .to_string()
}

fn calculate_request(employer_id: &str, run_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!(
            "/api/employers/{employer_id}/payroll-runs/{run_id}/calculate"
        ))
        .header(header::COOKIE, cookie)
        .header("x-salt-request", "1")
        .body(Body::empty())
        .unwrap()
}

fn finalize_request(employer_id: &str, run_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!(
            "/api/employers/{employer_id}/payroll-runs/{run_id}/finalize"
        ))
        .header(header::COOKIE, cookie)
        .header("x-salt-request", "1")
        .body(Body::empty())
        .unwrap()
}

fn record_compensation_terms_request(
    employer_id: &str,
    employment_id: &str,
    cookie: &str,
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
                "effectiveFrom": "2026-01-01",
                "basicPayCents": 1_500_000,
                "ordinaryHours": "40.00",
            })
            .to_string(),
        ))
        .unwrap()
}

fn declare_prior_employment_request(
    employer_id: &str,
    employment_id: &str,
    cookie: &str,
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
                "taxYear": 2025,
                "status": "confirmed_none",
            })
            .to_string(),
        ))
        .unwrap()
}

fn declare_unsupported_deductions_request(
    employer_id: &str,
    employment_id: &str,
    cookie: &str,
) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!(
            "/api/employers/{employer_id}/employments/{employment_id}/unsupported-deductions"
        ))
        .header(header::COOKIE, cookie)
        .header("x-salt-request", "1")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({
                "effectiveFrom": "2026-01-01",
                "status": "confirmed_none",
                "reason": "no unsupported deductions",
            })
            .to_string(),
        ))
        .unwrap()
}

fn set_employer_particulars_request(employer_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(format!("/api/employers/{employer_id}/particulars"))
        .header(header::COOKIE, cookie)
        .header("x-salt-request", "1")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({
                "registeredName": "Acme Corp (Pty) Ltd",
                "addressLine1": "1 Independence Ave",
                "city": "Windhoek",
                "acknowledgedDivergingPeriods": [],
                "reason": "",
            })
            .to_string(),
        ))
        .unwrap()
}

/// Declares every fact `calculate` needs for one Employment, over HTTP.
async fn declare_every_fact(employer_id: &str, employment_id: &str, cookie: &str) {
    let response = router()
        .await
        .oneshot(record_compensation_terms_request(
            employer_id,
            employment_id,
            cookie,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = router()
        .await
        .oneshot(declare_prior_employment_request(
            employer_id,
            employment_id,
            cookie,
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
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

/// Creates two fully-declared Employments, then creates, calculates and
/// finalizes one Ordinary run over both — the shortest path to a Register
/// with two rows (step 2's test 1). Returns
/// `(payrollRunId, [finalizedPayrollId; 2])`, in `employmentId` order.
async fn finalize_a_run_of_two(employer_id: &str, cookie: &str) -> (String, [String; 2]) {
    let employment_a = create_employment(employer_id, cookie, "Ada Lovelace").await;
    declare_every_fact(employer_id, &employment_a, cookie).await;
    let employment_b = create_employment(employer_id, cookie, "Bob Marley").await;
    declare_every_fact(employer_id, &employment_b, cookie).await;

    let run_id = create_run(employer_id, cookie).await;

    let response = router()
        .await
        .oneshot(calculate_request(employer_id, &run_id, cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = router()
        .await
        .oneshot(finalize_request(employer_id, &run_id, cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let finalized = body_json(response).await;
    let finalized = finalized["finalized"].as_array().unwrap().clone();
    assert_eq!(finalized.len(), 2);

    let finalized_payroll_id = |employment_id: &str| {
        finalized
            .iter()
            .find(|member| member["employmentId"] == employment_id)
            .unwrap()["finalizedPayrollId"]
            .as_str()
            .unwrap()
            .to_string()
    };

    (
        run_id,
        [
            finalized_payroll_id(&employment_a),
            finalized_payroll_id(&employment_b),
        ],
    )
}

fn register_request(employer_id: &str, run_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!(
            "/api/employers/{employer_id}/payroll-runs/{run_id}/register"
        ))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn payment_summary_request(employer_id: &str, run_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!(
            "/api/employers/{employer_id}/payroll-runs/{run_id}/payment-summary"
        ))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn register_pdf_request(employer_id: &str, run_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!(
            "/api/employers/{employer_id}/payroll-runs/{run_id}/register.pdf"
        ))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn payment_summary_pdf_request(employer_id: &str, run_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!(
            "/api/employers/{employer_id}/payroll-runs/{run_id}/payment-summary.pdf"
        ))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

/// `N$ 12,345.67`, matching `pdf_layout::format_money` exactly — copied
/// rather than shared, the same per-file fixture discipline this file's own
/// header comment already follows for `tests/payslip.rs`'s fixtures.
fn format_money(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let whole = cents.unsigned_abs() / 100;
    let fraction = cents.unsigned_abs() % 100;
    let digits = whole.to_string();
    let mut grouped = String::new();
    for (index, ch) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    let grouped: String = grouped.chars().rev().collect();
    format!("N$ {sign}{grouped}.{fraction:02}")
}

/// Test 1: an Operator gets a 200 register with two rows and both totals
/// equal (nothing has been reversed yet).
#[tokio::test]
async fn an_operator_gets_the_register_of_a_finalized_run() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let response = router()
        .await
        .oneshot(set_employer_particulars_request(&employer_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let (run_id, [finalized_a, finalized_b]) = finalize_a_run_of_two(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(register_request(&employer_id, &run_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;

    assert_eq!(body["payrollRunId"], run_id);
    assert_eq!(body["kind"], "ordinary");
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    let ids: Vec<&str> = rows
        .iter()
        .map(|row| row["finalizedPayrollId"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&finalized_a.as_str()));
    assert!(ids.contains(&finalized_b.as_str()));
    for row in rows {
        assert_eq!(row["liveness"]["state"], "live");
        assert_eq!(row["replaces"], Value::Null);
    }
    assert_eq!(
        body["totalAsFinalized"]["netCents"],
        body["totalStillLive"]["netCents"]
    );
}

fn native_january_period() -> payroll::PayPeriod {
    payroll::PayPeriod::new(
        NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
        NaiveDate::from_ymd_opt(2026, 1, 31).unwrap(),
    )
    .unwrap()
}

async fn a_fully_declared_employment_via_payroll_app(
    db: &SaltDatabase,
    employer_id: &payroll::EmployerId,
    person: &str,
) -> payroll::EmploymentId {
    let (_, employment_id) = payroll_app::create_employment(
        db,
        employer_id,
        EmploymentPerson::New(person.to_string()),
        native_january_period().start(),
        None,
        "actor",
    )
    .await
    .unwrap();
    payroll_app::record_compensation_terms(
        db,
        &employment_id,
        native_january_period().start(),
        payroll::Money::from_cents(1_500_000).unwrap(),
        payroll::OrdinaryHours::new(rust_decimal::Decimal::new(4_000, 2)).unwrap(),
        &[],
        "",
        "actor",
    )
    .await
    .unwrap();
    payroll_app::declare_prior_employment(
        db,
        &employment_id,
        payroll::TaxYear::for_period_end(native_january_period().end()),
        payroll::PriorEmployment::None,
        "actor",
    )
    .await
    .unwrap();
    payroll_app::declare_unsupported_deduction_status(
        db,
        &employment_id,
        native_january_period().start(),
        payroll::UnsupportedDeductionStatus::ConfirmedNone,
        &[],
        "a reason",
        "actor",
    )
    .await
    .unwrap();
    employment_id
}

/// Creates two fully-declared Employments and one finalized Ordinary run
/// over both, driven entirely through `payroll_app` rather than HTTP: the
/// reversal test below needs the typed `FinalizedPayrollId`
/// `reverse_finalized_payroll` takes, which (by design, see
/// `payroll-app/src/ids.rs`) has no public constructor from a bare string,
/// so it cannot be rebuilt from an HTTP JSON response the way
/// [`finalize_a_run_of_two`]'s plain `String` ids can.
async fn finalize_a_run_of_two_via_payroll_app(
    db: &SaltDatabase,
    employer_id: &payroll::EmployerId,
) -> (
    payroll_app::PayrollRunId,
    [(payroll::EmploymentId, payroll_app::FinalizedPayrollId); 2],
) {
    let employment_a =
        a_fully_declared_employment_via_payroll_app(db, employer_id, "Ada Lovelace").await;
    let employment_b =
        a_fully_declared_employment_via_payroll_app(db, employer_id, "Bob Marley").await;

    let run_id = payroll_app::create_ordinary_payroll_run(
        db,
        employer_id,
        native_january_period(),
        NaiveDate::from_ymd_opt(2026, 2, 5).unwrap(),
        "actor",
    )
    .await
    .unwrap();
    payroll_app::calculate_payroll_run(db, &run_id, "calculator")
        .await
        .unwrap();
    let outcome = payroll_app::finalize_payroll_run(db, &run_id, "finalizer")
        .await
        .unwrap();
    assert_eq!(outcome.finalized.len(), 2);

    let finalized_of = |employment_id: &payroll::EmploymentId| {
        outcome
            .finalized
            .iter()
            .find(|(id, _)| id == employment_id)
            .unwrap()
            .1
            .clone()
    };
    let finalized_a = finalized_of(&employment_a);
    let finalized_b = finalized_of(&employment_b);

    (
        run_id,
        [(employment_a, finalized_a), (employment_b, finalized_b)],
    )
}

/// Tests 2 and 3: after a reversal, the register's two totals diverge by
/// exactly the reversed row's net pay, that row shows `state: "reversed"`,
/// and the payment summary drops it while reporting
/// `excludedReversedCount: 1`.
#[tokio::test]
async fn a_reversal_splits_the_register_totals_and_shrinks_the_payment_summary() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let response = router()
        .await
        .oneshot(set_employer_particulars_request(&employer_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let db = test_db().await;
    let employer_id_typed = payroll::EmployerId::new(employer_id.clone());
    let (run_id, [(_employment_a, finalized_a), (_employment_b, _finalized_b)]) =
        finalize_a_run_of_two_via_payroll_app(&db, &employer_id_typed).await;
    let run_id = run_id.to_string();
    let finalized_a_str = finalized_a.to_string();

    payroll_app::reverse_finalized_payroll(&db, &finalized_a, "paid in error", "actor")
        .await
        .unwrap();
    let finalized_a = finalized_a_str;

    let response = router()
        .await
        .oneshot(register_request(&employer_id, &run_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;

    let reversed_row = body["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["finalizedPayrollId"] == finalized_a)
        .unwrap();
    assert_eq!(reversed_row["liveness"]["state"], "reversed");
    assert_eq!(reversed_row["liveness"]["reason"], "paid in error");
    assert!(reversed_row["liveness"]["reversedAt"].is_string());
    assert_eq!(reversed_row["liveness"]["replacedBy"], Value::Null);

    let total_as_finalized = body["totalAsFinalized"]["netCents"].as_i64().unwrap();
    let total_still_live = body["totalStillLive"]["netCents"].as_i64().unwrap();
    let reversed_net = reversed_row["figures"]["netCents"].as_i64().unwrap();
    assert_eq!(total_as_finalized - total_still_live, reversed_net);

    let response = router()
        .await
        .oneshot(payment_summary_request(&employer_id, &run_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;

    assert_eq!(body["excludedReversedCount"], 1);
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        !rows
            .iter()
            .any(|row| row["finalizedPayrollId"] == finalized_a)
    );
}

async fn finalize_january_via_payroll_app(
    db: &SaltDatabase,
    employer_id: &payroll::EmployerId,
    employment_id: &payroll::EmploymentId,
) -> payroll_app::FinalizedPayrollId {
    let run_id = payroll_app::create_ordinary_payroll_run(
        db,
        employer_id,
        native_january_period(),
        NaiveDate::from_ymd_opt(2026, 2, 5).unwrap(),
        "actor",
    )
    .await
    .unwrap();
    payroll_app::calculate_payroll_run(db, &run_id, "calculator")
        .await
        .unwrap();
    let outcome = payroll_app::finalize_payroll_run(db, &run_id, "finalizer")
        .await
        .unwrap();
    outcome
        .finalized
        .into_iter()
        .find(|(id, _)| id == employment_id)
        .expect("the Employment must have finalized")
        .1
}

/// Test 4: a Replacement row in a Correction run's payment summary carries
/// `replaces` and its own full net pay, never a difference.
#[tokio::test]
async fn a_replacement_row_carries_full_net_pay_and_replaces() {
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
    payroll_app::set_employer_particulars(
        &db,
        &employer_id,
        EmployerParticularsFields {
            registered_name: "Acme Corp (Pty) Ltd".to_string(),
            address_line1: "1 Independence Ave".to_string(),
            address_line2: None,
            city: "Windhoek".to_string(),
            postal_code: None,
            income_tax_number: None,
            social_security_number: None,
        },
        &[],
        "",
        "actor",
    )
    .await
    .unwrap();
    let cookie = login(&email).await;

    let employment_id =
        a_fully_declared_employment_via_payroll_app(&db, &employer_id, "Ada Lovelace").await;
    let original_id = finalize_january_via_payroll_app(&db, &employer_id, &employment_id).await;

    payroll_app::reverse_finalized_payroll(&db, &original_id, "salary was wrong", "actor")
        .await
        .unwrap();

    let correction_run_id = payroll_app::create_correction_run(
        &db,
        &employer_id,
        native_january_period(),
        NaiveDate::from_ymd_opt(2026, 4, 5).unwrap(),
        "the figure was wrong",
        "actor",
    )
    .await
    .unwrap();
    payroll_app::add_employment_to_correction_run(
        &db,
        &correction_run_id,
        &employment_id,
        Some(&original_id),
        "actor",
    )
    .await
    .unwrap();
    payroll_app::calculate_payroll_run(&db, &correction_run_id, "calculator")
        .await
        .unwrap();
    let outcome = payroll_app::finalize_payroll_run(&db, &correction_run_id, "finalizer")
        .await
        .unwrap();
    let replacement_id = outcome
        .finalized
        .into_iter()
        .find(|(id, _)| *id == employment_id)
        .expect("the Correction's one member must have finalized")
        .1;

    let employer_id = employer_id.to_string();
    let correction_run_id = correction_run_id.to_string();

    let response = router()
        .await
        .oneshot(payment_summary_request(
            &employer_id,
            &correction_run_id,
            &cookie,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;

    assert_eq!(body["kind"], "correction");
    let rows = body["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row["finalizedPayrollId"], replacement_id.to_string());
    assert_eq!(row["replaces"], original_id.to_string());
    assert!(row["netPayCents"].as_i64().unwrap() > 0);
}

/// Test 5: a payroll finalized with no frozen `EmployerParticulars` still
/// answers both routes with 200 — unlike `payslip.pdf`, which is 409
/// `payslip_particulars_not_frozen` for the same id.
#[tokio::test]
async fn both_routes_answer_200_without_frozen_particulars() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    declare_every_fact(&employer_id, &employment_id, &cookie).await;
    let run_id = create_run(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(calculate_request(&employer_id, &run_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = router()
        .await
        .oneshot(finalize_request(&employer_id, &run_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let finalized = body_json(response).await;
    let finalized_payroll_id = finalized["finalized"][0]["finalizedPayrollId"]
        .as_str()
        .unwrap()
        .to_string();

    let response = router()
        .await
        .oneshot(register_request(&employer_id, &run_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = router()
        .await
        .oneshot(payment_summary_request(&employer_id, &run_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = router()
        .await
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/employers/{employer_id}/finalized-payroll/{finalized_payroll_id}/payslip.pdf"
                ))
                .header(header::COOKIE, &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "payslip_particulars_not_frozen"
    );
}

/// Test 6: a Draft (never calculated or finalized) run answers 409
/// `payroll_run_not_finalized` on both routes.
#[tokio::test]
async fn a_not_finalized_run_is_refused_on_both_routes() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let run_id = create_run(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(register_request(&employer_id, &run_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "payroll_run_not_finalized"
    );

    let response = router()
        .await
        .oneshot(payment_summary_request(&employer_id, &run_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "payroll_run_not_finalized"
    );
}

/// Test 7: a run id belonging to another Employer answers 404, identical to
/// a random uuid, on both routes (ADR-0017).
#[tokio::test]
async fn a_cross_employer_run_id_is_not_found_like_an_unknown_one_on_both_routes() {
    let (_owning_email, owning_cookie, owning_employer) = an_authorized_operator().await;
    let (_other_email, other_cookie, other_employer) = an_authorized_operator().await;
    let response = router()
        .await
        .oneshot(set_employer_particulars_request(
            &owning_employer,
            &owning_cookie,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let (run_id, _finalized) = finalize_a_run_of_two(&owning_employer, &owning_cookie).await;
    let unknown_id = uuid::Uuid::new_v4().to_string();

    for (route_request, label) in [
        (
            register_request as fn(&str, &str, &str) -> Request<Body>,
            "register",
        ),
        (payment_summary_request, "payment-summary"),
    ] {
        let cross_employer_response = router()
            .await
            .oneshot(route_request(&other_employer, &run_id, &other_cookie))
            .await
            .unwrap();
        let unknown_response = router()
            .await
            .oneshot(route_request(&owning_employer, &unknown_id, &owning_cookie))
            .await
            .unwrap();

        assert_eq!(
            cross_employer_response.status(),
            StatusCode::NOT_FOUND,
            "{label}"
        );
        assert_eq!(unknown_response.status(), StatusCode::NOT_FOUND, "{label}");
        let cross_employer_body = body_json(cross_employer_response).await;
        let unknown_body = body_json(unknown_response).await;
        assert_eq!(
            cross_employer_body["error"]["code"], unknown_body["error"]["code"],
            "{label}"
        );
        assert_eq!(
            cross_employer_body["error"]["code"],
            "payroll_run_not_found"
        );
    }
}

/// Test 8: no session is 401 on both routes; a `PayrollOperator` (not just
/// an Owner) reads 200 on both, the same "either role" rule the finalized
/// payroll detail route already follows (§0.6).
#[tokio::test]
async fn no_session_is_401_and_a_payroll_operator_reads_200_on_both_routes() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let response = router()
        .await
        .oneshot(set_employer_particulars_request(&employer_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let (run_id, _finalized) = finalize_a_run_of_two(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/employers/{employer_id}/payroll-runs/{run_id}/register"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = router()
        .await
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/employers/{employer_id}/payroll-runs/{run_id}/payment-summary"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let db = test_db().await;
    let operator_email = unique_email("bob");
    let operator_id = create_operator(&operator_email).await;
    let employer_id_typed = payroll::EmployerId::new(employer_id.clone());
    payroll_app::create_employer_membership(
        &db,
        &operator_id,
        &employer_id_typed,
        MembershipRole::PayrollOperator,
    )
    .await
    .unwrap();
    let operator_cookie = login(&operator_email).await;

    let response = router()
        .await
        .oneshot(register_request(&employer_id, &run_id, &operator_cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = router()
        .await
        .oneshot(payment_summary_request(
            &employer_id,
            &run_id,
            &operator_cookie,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

async fn body_bytes(response: axum::response::Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

/// Both PDFs download as uncacheable PDFs, the same contract `payslip.pdf`
/// already carries (issue #82): `application/pdf`, `Cache-Control:
/// no-store`, and real PDF bytes.
#[tokio::test]
async fn register_and_payment_summary_pdfs_download_as_uncacheable_pdfs() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let response = router()
        .await
        .oneshot(set_employer_particulars_request(&employer_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let (run_id, _finalized) = finalize_a_run_of_two(&employer_id, &cookie).await;

    for request_fn in [
        register_pdf_request as fn(&str, &str, &str) -> Request<Body>,
        payment_summary_pdf_request,
    ] {
        let response = router()
            .await
            .oneshot(request_fn(&employer_id, &run_id, &cookie))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/pdf"
        );
        assert_eq!(
            response.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
        let bytes = body_bytes(response).await;
        assert!(bytes.starts_with(b"%PDF"));
    }
}

/// The figures a PDF prints are exactly the figures its own JSON route
/// answers — every row's net pay, and both routes' own totals — read back
/// out of the rendered PDF text rather than assumed.
#[tokio::test]
async fn pdf_figures_match_the_json_routes_figures() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let response = router()
        .await
        .oneshot(set_employer_particulars_request(&employer_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let (run_id, _finalized) = finalize_a_run_of_two(&employer_id, &cookie).await;

    let json_register = body_json(
        router()
            .await
            .oneshot(register_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    let register_bytes = body_bytes(
        router()
            .await
            .oneshot(register_pdf_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    let register_text = rendered_text(&register_bytes);
    assert!(
        register_text.contains(&format_money(
            json_register["totalAsFinalized"]["netCents"]
                .as_i64()
                .unwrap()
        )),
        "{register_text}"
    );
    for row in json_register["rows"].as_array().unwrap() {
        let net_cents = row["figures"]["netCents"].as_i64().unwrap();
        assert!(
            register_text.contains(&format_money(net_cents)),
            "{register_text}"
        );
    }

    let json_summary = body_json(
        router()
            .await
            .oneshot(payment_summary_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    let summary_bytes = body_bytes(
        router()
            .await
            .oneshot(payment_summary_pdf_request(&employer_id, &run_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    let summary_text = rendered_text(&summary_bytes);
    assert!(
        summary_text.contains(&format_money(
            json_summary["totalNetPayCents"].as_i64().unwrap()
        )),
        "{summary_text}"
    );
    for row in json_summary["rows"].as_array().unwrap() {
        let net_cents = row["netPayCents"].as_i64().unwrap();
        assert!(
            summary_text.contains(&format_money(net_cents)),
            "{summary_text}"
        );
    }
}

/// A pre-#73 style row (never froze `EmployerParticulars`, and finalized
/// with no particulars declared at all — the same shape
/// `both_routes_answer_200_without_frozen_particulars` proves for the JSON
/// routes) still answers 200 on both PDF routes: unlike `payslip.pdf`, a
/// Register or Payment Summary needs only figures, never a frozen
/// particular (README.md acceptance criterion 6).
#[tokio::test]
async fn both_pdf_routes_answer_200_without_frozen_particulars() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    declare_every_fact(&employer_id, &employment_id, &cookie).await;
    let run_id = create_run(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(calculate_request(&employer_id, &run_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response = router()
        .await
        .oneshot(finalize_request(&employer_id, &run_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    for request_fn in [
        register_pdf_request as fn(&str, &str, &str) -> Request<Body>,
        payment_summary_pdf_request,
    ] {
        let response = router()
            .await
            .oneshot(request_fn(&employer_id, &run_id, &cookie))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}

/// A Draft (never calculated or finalized) run answers 409
/// `payroll_run_not_finalized` on both PDF routes, the same refusal the JSON
/// routes give.
#[tokio::test]
async fn a_not_finalized_run_is_refused_on_both_pdf_routes() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let run_id = create_run(&employer_id, &cookie).await;

    for request_fn in [
        register_pdf_request as fn(&str, &str, &str) -> Request<Body>,
        payment_summary_pdf_request,
    ] {
        let response = router()
            .await
            .oneshot(request_fn(&employer_id, &run_id, &cookie))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            body_json(response).await["error"]["code"],
            "payroll_run_not_finalized"
        );
    }
}

/// A run id belonging to another Employer answers 404, identical to a
/// random uuid, on both PDF routes (ADR-0017) — the same isolation the JSON
/// routes already prove.
#[tokio::test]
async fn a_cross_employer_run_id_is_not_found_like_an_unknown_one_on_both_pdf_routes() {
    let (_owning_email, owning_cookie, owning_employer) = an_authorized_operator().await;
    let (_other_email, other_cookie, other_employer) = an_authorized_operator().await;
    let response = router()
        .await
        .oneshot(set_employer_particulars_request(
            &owning_employer,
            &owning_cookie,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let (run_id, _finalized) = finalize_a_run_of_two(&owning_employer, &owning_cookie).await;
    let unknown_id = uuid::Uuid::new_v4().to_string();

    for (route_request, label) in [
        (
            register_pdf_request as fn(&str, &str, &str) -> Request<Body>,
            "register.pdf",
        ),
        (payment_summary_pdf_request, "payment-summary.pdf"),
    ] {
        let cross_employer_response = router()
            .await
            .oneshot(route_request(&other_employer, &run_id, &other_cookie))
            .await
            .unwrap();
        let unknown_response = router()
            .await
            .oneshot(route_request(&owning_employer, &unknown_id, &owning_cookie))
            .await
            .unwrap();

        assert_eq!(
            cross_employer_response.status(),
            StatusCode::NOT_FOUND,
            "{label}"
        );
        assert_eq!(unknown_response.status(), StatusCode::NOT_FOUND, "{label}");
        let cross_employer_body = body_json(cross_employer_response).await;
        let unknown_body = body_json(unknown_response).await;
        assert_eq!(
            cross_employer_body["error"]["code"], unknown_body["error"]["code"],
            "{label}"
        );
        assert_eq!(
            cross_employer_body["error"]["code"],
            "payroll_run_not_found"
        );
    }
}

/// No session is 401 on both PDF routes, the same guard every other payroll
/// route carries.
#[tokio::test]
async fn no_session_is_401_on_both_pdf_routes() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let response = router()
        .await
        .oneshot(set_employer_particulars_request(&employer_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let (run_id, _finalized) = finalize_a_run_of_two(&employer_id, &cookie).await;

    for request_fn in [
        register_pdf_request as fn(&str, &str, &str) -> Request<Body>,
        payment_summary_pdf_request,
    ] {
        let response = router()
            .await
            .oneshot(request_fn(&employer_id, &run_id, ""))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
