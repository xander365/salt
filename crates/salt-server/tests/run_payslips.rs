//! Proves `GET /api/employers/{e}/payroll-runs/{r}/payslips.pdf` (issue
//! #83, parent #70): every payslip a finalized run produced, one PDF, each
//! starting on a new page, in member order. Driven with
//! `tower::ServiceExt::oneshot` against the real router, the same
//! discipline `tests/payslip.rs` already follows — fixtures are copied
//! rather than shared, matching this codebase's existing per-file fixture
//! discipline.

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::NaiveDate;
use payroll_app::{
    DatabaseConfig, EmployerParticularsFields, EmploymentPerson, MembershipRole, OperatorId,
    SaltDatabase,
};
use salt_server::{AppState, TextPlacement, build_router, rendered_text, text_placements};
use serde_json::Value;
use tower::ServiceExt;

fn test_database_config() -> DatabaseConfig {
    let url = std::env::var("DATABASE_URL").expect(
        "DATABASE_URL must name an already-migrated database for salt-server's own tests \
         (see crates/salt-server/tests/payslip.rs)",
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

/// Declares every fact `calculate` needs for `employment_id`, over HTTP.
/// Does not create, calculate or finalize a run — callers batch several
/// employments onto one run before doing that, unlike
/// `tests/payslip.rs`'s own single-employment helper.
async fn fully_declare_employment(employer_id: &str, employment_id: &str, cookie: &str) {
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

/// Calculates and finalizes `run_id`, returning the full finalize response
/// body — every member's `employmentId` alongside its `finalizedPayrollId`,
/// so a caller can look a specific member's id up rather than trust array
/// order.
async fn calculate_and_finalize_run(employer_id: &str, run_id: &str, cookie: &str) -> Value {
    let response = router()
        .await
        .oneshot(calculate_request(employer_id, run_id, cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = router()
        .await
        .oneshot(finalize_request(employer_id, run_id, cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await
}

fn finalized_payroll_id_for(finalized: &Value, employment_id: &str) -> String {
    finalized["finalized"]
        .as_array()
        .expect("finalize's response always carries a `finalized` array")
        .iter()
        .find(|entry| entry["employmentId"] == employment_id)
        .unwrap_or_else(|| {
            panic!("no finalized member for employment {employment_id}: {finalized}")
        })["finalizedPayrollId"]
        .as_str()
        .unwrap()
        .to_string()
}

fn run_payslips_request(employer_id: &str, run_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!(
            "/api/employers/{employer_id}/payroll-runs/{run_id}/payslips.pdf"
        ))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn payslip_request(employer_id: &str, finalized_payroll_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!(
            "/api/employers/{employer_id}/finalized-payroll/{finalized_payroll_id}/payslip.pdf"
        ))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

async fn pdf_bytes(request: Request<Body>) -> Vec<u8> {
    let response = router().await.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/pdf"
    );
    assert_eq!(
        response.headers().get(header::CACHE_CONTROL).unwrap(),
        "no-store"
    );
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

/// Every placement's text on one page of a batch PDF, joined the same way
/// [`rendered_text`] joins a whole document — for comparing one payslip's
/// own page against a single-payslip render.
fn text_on_page(placements: &[TextPlacement], page: usize) -> String {
    placements
        .iter()
        .filter(|placement| placement.page == page)
        .map(|placement| placement.text.clone())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Two members, both finalized on one Ordinary run — the shortest path to a
/// batch worth paginating.
async fn two_member_run(employer_id: &str, cookie: &str) -> (String, String, String) {
    let response = router()
        .await
        .oneshot(set_employer_particulars_request(employer_id, cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let ada_id = create_employment(employer_id, cookie, "Ada Lovelace").await;
    fully_declare_employment(employer_id, &ada_id, cookie).await;
    let bob_id = create_employment(employer_id, cookie, "Bob Marley").await;
    fully_declare_employment(employer_id, &bob_id, cookie).await;

    let run_id = create_run(employer_id, cookie).await;
    let finalized = calculate_and_finalize_run(employer_id, &run_id, cookie).await;
    let ada_finalized_id = finalized_payroll_id_for(&finalized, &ada_id);
    let bob_finalized_id = finalized_payroll_id_for(&finalized, &bob_id);

    (run_id, ada_finalized_id, bob_finalized_id)
}

/// The everyday case: two members, one PDF, each on its own page, in member
/// order (`employment.id` — Ada was created first).
#[tokio::test]
async fn two_members_print_in_member_order_each_starting_a_new_page() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let (run_id, _ada_finalized_id, _bob_finalized_id) =
        two_member_run(&employer_id, &cookie).await;

    let bytes = pdf_bytes(run_payslips_request(&employer_id, &run_id, &cookie)).await;
    assert!(bytes.starts_with(b"%PDF"));

    let text = rendered_text(&bytes);
    assert!(text.contains("Ada Lovelace"), "{text}");
    assert!(text.contains("Bob Marley"), "{text}");

    let placements = text_placements(&bytes);
    let ada_first_page = placements
        .iter()
        .find(|placement| placement.text.contains("Ada"))
        .expect("Ada's name is printed somewhere")
        .page;
    let bob_first_page = placements
        .iter()
        .find(|placement| placement.text.contains("Bob"))
        .expect("Bob's name is printed somewhere")
        .page;
    assert!(
        ada_first_page < bob_first_page,
        "Ada (member order first) must print before Bob: {ada_first_page} vs {bob_first_page}"
    );

    let ada_max_page = placements
        .iter()
        .filter(|placement| placement.page == ada_first_page)
        .map(|placement| placement.page)
        .max()
        .unwrap();
    assert!(
        bob_first_page > ada_max_page,
        "Bob's payslip must start on a fresh page, never share Ada's own"
    );
}

/// Each payslip's extracted text inside the batch is exactly the same
/// content a single-payslip download would print for that same id —
/// content, never bytes (ADR-0021's own promise, extended to a batch).
#[tokio::test]
async fn each_batch_payslip_matches_its_own_single_payslip_content() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let (run_id, ada_finalized_id, bob_finalized_id) = two_member_run(&employer_id, &cookie).await;

    let batch_bytes = pdf_bytes(run_payslips_request(&employer_id, &run_id, &cookie)).await;
    let batch_placements = text_placements(&batch_bytes);

    let ada_single = pdf_bytes(payslip_request(&employer_id, &ada_finalized_id, &cookie)).await;
    let bob_single = pdf_bytes(payslip_request(&employer_id, &bob_finalized_id, &cookie)).await;

    assert_eq!(
        text_on_page(&batch_placements, 0),
        rendered_text(&ada_single)
    );
    assert_eq!(
        text_on_page(&batch_placements, 1),
        rendered_text(&bob_single)
    );
}

/// A reversed row in the batch still carries its own REVERSED marking —
/// batching never drops the single-payslip content contract.
#[tokio::test]
async fn a_reversed_row_in_the_batch_still_prints_reversed() {
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
    let run_id = payroll_app::create_ordinary_payroll_run(
        &db,
        &employer_id,
        native_january_period(),
        NaiveDate::from_ymd_opt(2026, 2, 5).unwrap(),
        "actor",
    )
    .await
    .unwrap();
    payroll_app::calculate_payroll_run(&db, &run_id, "calculator")
        .await
        .unwrap();
    let outcome = payroll_app::finalize_payroll_run(&db, &run_id, "finalizer")
        .await
        .unwrap();
    let finalized_payroll_id = outcome
        .finalized
        .into_iter()
        .find(|(id, _)| *id == employment_id)
        .expect("the Employment must have finalized")
        .1;

    payroll_app::reverse_finalized_payroll(
        &db,
        &finalized_payroll_id,
        "March salary was wrong",
        "actor",
    )
    .await
    .unwrap();

    let run_id = run_id.to_string();
    let employer_id = employer_id.to_string();

    let bytes = pdf_bytes(run_payslips_request(&employer_id, &run_id, &cookie)).await;
    let text = rendered_text(&bytes);
    assert!(text.contains("REVERSED"), "{text}");
    assert!(text.contains("March salary was wrong"), "{text}");
}

/// A CorrectionRun's batch has exactly one payslip — its single member —
/// marked REPLACEMENT.
#[tokio::test]
async fn a_correction_runs_batch_has_one_payslip_marked_replacement() {
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
    payroll_app::reverse_finalized_payroll(&db, &original_id, "March salary was wrong", "actor")
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
    payroll_app::finalize_payroll_run(&db, &correction_run_id, "finalizer")
        .await
        .unwrap();

    let employer_id = employer_id.to_string();
    let original_id = original_id.to_string();
    let run_id = correction_run_id.to_string();

    let bytes = pdf_bytes(run_payslips_request(&employer_id, &run_id, &cookie)).await;
    let text = rendered_text(&bytes);
    assert!(text.contains("REPLACEMENT"), "{text}");
    assert!(
        text.contains(&format!(
            "This payslip replaces finalized payroll {original_id}"
        )),
        "{text}"
    );
    assert!(!text.contains("REVERSED"), "{text}");

    // Exactly one payslip: only page 0 exists.
    let placements = text_placements(&bytes);
    assert_eq!(
        placements.iter().map(|placement| placement.page).max(),
        Some(0),
        "a Correction's single member must print exactly one page"
    );
}

/// A member with no frozen Employer particulars refuses the whole batch,
/// naming exactly what is missing — the same refusal a single payslip gives
/// (`tests/payslip.rs`), extended to "the whole batch" rather than only
/// that one row.
#[tokio::test]
async fn a_member_with_no_frozen_particulars_refuses_the_whole_batch() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    // Deliberately no `set_employer_particulars_request` call.
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    fully_declare_employment(&employer_id, &employment_id, &cookie).await;
    let run_id = create_run(&employer_id, &cookie).await;
    calculate_and_finalize_run(&employer_id, &run_id, &cookie).await;

    let response = router()
        .await
        .oneshot(run_payslips_request(&employer_id, &run_id, &cookie))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body = body_json(response).await;
    assert_eq!(body["error"]["code"], "payslip_particulars_not_frozen");
    assert_eq!(
        body["error"]["details"]["missing"],
        serde_json::json!(["EmployerParticulars"])
    );
}

/// A Draft (never calculated, never finalized) run has no `FinalizedPayroll`
/// rows for a batch to print — refused, never an empty-looking 200.
#[tokio::test]
async fn a_run_that_has_not_finalized_is_refused() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let run_id = create_run(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(run_payslips_request(&employer_id, &run_id, &cookie))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "payroll_run_not_finalized"
    );
}

/// ADR-0017: a run id belonging to another Employer, and an unknown one,
/// answer the same 404 — indistinguishable, never a hint either way.
#[tokio::test]
async fn a_run_id_belonging_to_another_employer_or_unknown_is_not_found() {
    let (_owning_email, owning_cookie, owning_employer) = an_authorized_operator().await;
    let (_other_email, other_cookie, other_employer) = an_authorized_operator().await;
    let (run_id, _ada, _bob) = two_member_run(&owning_employer, &owning_cookie).await;

    let response = router()
        .await
        .oneshot(run_payslips_request(
            &other_employer,
            &run_id,
            &other_cookie,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "payroll_run_not_found"
    );

    let unknown_id = uuid::Uuid::new_v4().to_string();
    let response = router()
        .await
        .oneshot(run_payslips_request(
            &owning_employer,
            &unknown_id,
            &owning_cookie,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "payroll_run_not_found"
    );
}

fn native_january_period() -> payroll::PayPeriod {
    payroll::PayPeriod::new(
        NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
        NaiveDate::from_ymd_opt(2026, 1, 31).unwrap(),
    )
    .unwrap()
}

/// As `tests/payslip.rs`'s own fixture of the same name: reversing and
/// replacing a `FinalizedPayroll` has no HTTP route, so this drives
/// `payroll_app` directly.
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
