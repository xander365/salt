//! Proves `GET /api/employers/{e}/finalized-payroll/{f}/payslip` (issue
//! #82, parent #70). Driven with `tower::ServiceExt::oneshot` against the
//! real router, the same discipline `tests/finalized_payroll.rs` already
//! follows — most of the fixtures below are copied from that file rather
//! than shared, matching this codebase's existing per-file fixture
//! discipline (`tests/correction_run.rs` and `tests/reverse_finalized_payroll.rs`
//! in `payroll-app` do the same).

use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use chrono::NaiveDate;
use payroll_app::{
    DatabaseConfig, EmployerParticularsFields, EmploymentPerson, MembershipRole, OperatorId,
    SaltDatabase,
};
use printpdf::{Op, PdfDocument, PdfParseOptions, PdfWarnMsg, TextItem};
use salt_server::{AppState, build_router};
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

/// Declares every fact `calculate` needs, and creates, calculates and
/// finalizes an Ordinary run — the same shortest path
/// `tests/finalized_payroll.rs` uses to reach one live `FinalizedPayroll`.
async fn finalize_a_fully_declared_employment(
    employer_id: &str,
    employment_id: &str,
    cookie: &str,
) -> String {
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
    finalized["finalized"][0]["finalizedPayrollId"]
        .as_str()
        .unwrap()
        .to_string()
}

fn payslip_request(employer_id: &str, finalized_payroll_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!(
            "/api/employers/{employer_id}/finalized-payroll/{finalized_payroll_id}/payslip"
        ))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

/// Extracts every printed text run from a rendered payslip, the same
/// mechanism `payslip_render`'s own unit tests use — Deep Instructions'
/// "verify by extracting the rendered values, never by hashing bytes",
/// applied here to the actual bytes an HTTP client would receive.
fn rendered_text(bytes: &[u8]) -> String {
    let mut warnings: Vec<PdfWarnMsg> = Vec::new();
    let doc = PdfDocument::parse(bytes, &PdfParseOptions::default(), &mut warnings)
        .expect("the payslip route must always answer a parseable PDF");
    let mut text = String::new();
    for page in &doc.pages {
        for op in &page.ops {
            if let Op::ShowText { items } = op {
                for item in items {
                    match item {
                        TextItem::Text(s) => text.push_str(s),
                        TextItem::GlyphIds(codepoints) => {
                            for codepoint in codepoints {
                                if let Some(cid) = &codepoint.cid {
                                    text.push_str(cid);
                                }
                            }
                        }
                        TextItem::Offset(_) => {}
                    }
                }
                text.push(' ');
            }
        }
    }
    text
}

/// The everyday case: every frozen particular is on record, so the payslip
/// downloads as a real PDF, never cacheable, carrying the frozen content.
#[tokio::test]
async fn an_operator_downloads_a_payslip_as_an_uncacheable_pdf() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;

    let response = router()
        .await
        .oneshot(set_employer_particulars_request(&employer_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let finalized_payroll_id =
        finalize_a_fully_declared_employment(&employer_id, &employment_id, &cookie).await;

    let response = router()
        .await
        .oneshot(payslip_request(
            &employer_id,
            &finalized_payroll_id,
            &cookie,
        ))
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
    let disposition = response
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        disposition.starts_with("attachment; filename=\""),
        "{disposition}"
    );
    assert!(disposition.contains(&finalized_payroll_id), "{disposition}");

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(bytes.starts_with(b"%PDF"));

    let text = rendered_text(&bytes);
    assert!(text.contains("Acme Corp (Pty) Ltd"), "{text}");
    assert!(text.contains("Ada Lovelace"), "{text}");
    assert!(text.contains("N$ 15,000.00"), "{text}");
    assert!(text.contains("standard-v1"), "{text}");
}

/// The refusal Deep Instructions demand: an Employer that never recorded
/// EmployerParticulars finalizes a payroll with nothing frozen to print, and
/// the payslip route refuses it naming exactly that — never a 404, never a
/// 500, and never a payslip missing an employer address.
#[tokio::test]
async fn a_payroll_with_no_frozen_employer_particulars_is_refused_naming_what_is_missing() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let finalized_payroll_id =
        finalize_a_fully_declared_employment(&employer_id, &employment_id, &cookie).await;

    let response = router()
        .await
        .oneshot(payslip_request(
            &employer_id,
            &finalized_payroll_id,
            &cookie,
        ))
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

#[tokio::test]
async fn a_finalized_payroll_id_belonging_to_another_employer_is_not_found() {
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
    let employment_id = create_employment(&owning_employer, &owning_cookie, "Ada Lovelace").await;
    let finalized_payroll_id =
        finalize_a_fully_declared_employment(&owning_employer, &employment_id, &owning_cookie)
            .await;

    let response = router()
        .await
        .oneshot(payslip_request(
            &other_employer,
            &finalized_payroll_id,
            &other_cookie,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "finalized_payroll_not_found"
    );
}

#[tokio::test]
async fn an_unknown_finalized_payroll_id_is_not_found() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let unknown_id = uuid::Uuid::new_v4().to_string();

    let response = router()
        .await
        .oneshot(payslip_request(&employer_id, &unknown_id, &cookie))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "finalized_payroll_not_found"
    );
}

#[tokio::test]
async fn downloading_a_payslip_requires_a_session() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let response = router()
        .await
        .oneshot(set_employer_particulars_request(&employer_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let finalized_payroll_id =
        finalize_a_fully_declared_employment(&employer_id, &employment_id, &cookie).await;

    let response = router()
        .await
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/employers/{employer_id}/finalized-payroll/{finalized_payroll_id}/payslip"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

/// A `PayrollOperator` (not just an `Owner`) can download a payslip too —
/// same "either role" rule the finalized-payroll detail route already
/// follows (§0.6).
#[tokio::test]
async fn a_payroll_operator_can_download_a_payslip() {
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

    // `PUT .../particulars` is Owner-only (§0.6): a `PayrollOperator` set up
    // via `payroll_app` directly here, the same way
    // `a_reversed_and_replaced_payroll_names_both_directions_on_its_payslip`
    // already reaches past an HTTP-only role restriction to set up state.
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

    let employer_id = employer_id.to_string();
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let finalized_payroll_id =
        finalize_a_fully_declared_employment(&employer_id, &employment_id, &cookie).await;

    let response = router()
        .await
        .oneshot(payslip_request(
            &employer_id,
            &finalized_payroll_id,
            &cookie,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

fn native_january_period() -> payroll::PayPeriod {
    payroll::PayPeriod::new(
        NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
        NaiveDate::from_ymd_opt(2026, 1, 31).unwrap(),
    )
    .unwrap()
}

/// An Employment with every fact `calculate` needs, built by calling
/// `payroll_app` directly rather than through HTTP — the same discipline
/// `payroll-app`'s own `tests/correction_run.rs` fixture follows. Needed
/// only by the reversal/replacement test below: reversing and replacing a
/// `FinalizedPayroll` has no HTTP route yet, so setting up that state must
/// go through `payroll_app`'s own typed `FinalizedPayrollId` — a value
/// [`payslip_request`]'s `&str` id can be built *from* (via `.to_string()`)
/// but never reconstructed *back into*, since that id has no public
/// constructor (by design — see `payroll-app/src/ids.rs`).
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

/// The full lineage, over HTTP (CONTEXT.md's own `Reversal`/`Replacement`
/// entries): reversing and replacing has no HTTP route of its own yet, so
/// this drives `payroll_app` directly to set up the state — the same
/// database the router itself reads from — and then asserts what the
/// payslip route, reached the ordinary way over HTTP, prints for each side
/// of the chain.
#[tokio::test]
async fn a_reversed_and_replaced_payroll_names_both_directions_on_its_payslip() {
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

    let run_id = payroll_app::create_correction_run(
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
        &run_id,
        &employment_id,
        Some(&original_id),
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
    let replacement_id = outcome
        .finalized
        .into_iter()
        .find(|(id, _)| *id == employment_id)
        .expect("the Correction's one member must have finalized")
        .1;

    let employer_id = employer_id.to_string();
    let original_id = original_id.to_string();
    let replacement_id = replacement_id.to_string();

    let original_response = router()
        .await
        .oneshot(payslip_request(&employer_id, &original_id, &cookie))
        .await
        .unwrap();
    assert_eq!(original_response.status(), StatusCode::OK);
    let original_bytes = axum::body::to_bytes(original_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let original_text = rendered_text(&original_bytes);
    assert!(original_text.contains("REVERSED"), "{original_text}");
    assert!(
        original_text.contains("March salary was wrong"),
        "{original_text}"
    );
    assert!(original_text.contains(&replacement_id), "{original_text}");

    let replacement_response = router()
        .await
        .oneshot(payslip_request(&employer_id, &replacement_id, &cookie))
        .await
        .unwrap();
    assert_eq!(replacement_response.status(), StatusCode::OK);
    let replacement_bytes = axum::body::to_bytes(replacement_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let replacement_text = rendered_text(&replacement_bytes);
    assert!(
        replacement_text.contains(&original_id),
        "{replacement_text}"
    );
}
