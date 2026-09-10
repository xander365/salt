//! Proves `GET /api/employers/{e}/finalized-payroll/{f}` and `GET
//! /api/employers/{e}/finalized-payroll/{f}/traces` (issue #57, parent #49
//! Spec 2 of 3). Driven with `tower::ServiceExt::oneshot` against the real
//! router, the same discipline `tests/payroll_runs.rs` already follows.

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
         (see crates/salt-server/tests/finalized_payroll.rs)",
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

/// Extracts the `salt_session=...` pair out of a `Set-Cookie` header, so a
/// later request can carry it back in its own `Cookie` header the way a
/// browser would.
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

fn logout_request(cookie: &str) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri("/api/session")
        .header("x-salt-request", "1")
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

/// An Operator, logged in, with an active `Owner` membership for a fresh
/// Employer on a calendar-month `PaySchedule` (period end is the last day of
/// the month). Returns the email, the cookie and the Employer's own id — the
/// email is returned too so a test can sign back in as the same Operator.
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

/// Declares every fact `calculate` needs for `employment_id` under
/// `january_period()`'s own `PaySchedule` and `TaxYear`, and creates,
/// calculates and finalizes an Ordinary run for it — the shortest path to
/// one live `FinalizedPayroll` this file's tests can read back. Returns the
/// finalized payroll id.
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

fn detail_request(employer_id: &str, finalized_payroll_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!(
            "/api/employers/{employer_id}/finalized-payroll/{finalized_payroll_id}"
        ))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn traces_request(employer_id: &str, finalized_payroll_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(format!(
            "/api/employers/{employer_id}/finalized-payroll/{finalized_payroll_id}/traces"
        ))
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn reading_a_finalized_payroll_returns_the_ten_figures_the_period_the_pay_date_and_the_salt_version()
 {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let finalized_payroll_id =
        finalize_a_fully_declared_employment(&employer_id, &employment_id, &cookie).await;

    let response = router()
        .await
        .oneshot(detail_request(&employer_id, &finalized_payroll_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;

    assert_eq!(body["finalizedPayrollId"], finalized_payroll_id);
    assert_eq!(body["employmentId"], employment_id);
    assert_eq!(body["fullName"], "Ada Lovelace");
    assert_eq!(body["period"]["start"], "2026-01-01");
    assert_eq!(body["period"]["end"], "2026-01-31");
    assert_eq!(body["payDate"], "2026-02-05");
    assert!(!body["saltVersion"].as_str().unwrap().is_empty());

    let figures = &body["figures"];
    assert_eq!(figures["basicPayCents"], 1_500_000);
    assert_eq!(figures["taxableAllowancesCents"], 0);
    assert_eq!(figures["overtimeCents"], 0);
    assert_eq!(figures["grossCents"], 1_500_000);
    assert_eq!(figures["taxableRemunerationCents"], 1_500_000);
    assert!(figures["payeCents"].is_i64());
    assert!(figures["employeeSscCents"].is_i64());
    assert!(figures["employerSscCents"].is_i64());
    assert!(figures["netCents"].is_i64());
    assert_eq!(
        figures["totalDeductionsCents"],
        figures["payeCents"].as_i64().unwrap() + figures["employeeSscCents"].as_i64().unwrap(),
        "totalDeductions is PAYE plus employee SSC, never employer SSC (INV-007)"
    );
    assert_eq!(
        figures["grossCents"].as_i64().unwrap() - figures["totalDeductionsCents"].as_i64().unwrap(),
        figures["netCents"].as_i64().unwrap()
    );

    // Never the raw snapshot (§0.29): none of the JSONB columns'
    // deserialized shape — earning_lines, a bands walk, a schedule, a
    // year_to_date context — leaks through as a top-level or nested key.
    assert!(body.get("payrollInputJson").is_none());
    assert!(body.get("payrollRulesJson").is_none());
    assert!(body.get("payrollCalculationJson").is_none());
    assert!(figures.get("earningLines").is_none());
    assert!(figures.get("trace").is_none());

    // Issue #73: neither particular was ever recorded here, so
    // `employerParticulars` reads back `null` — nothing was on record to
    // freeze — while `personParticulars` still freezes, because it always
    // carries at least a `fullName`. `payslipTemplateVersion` freezes
    // regardless of any master data.
    assert_eq!(body["employerParticulars"], Value::Null);
    assert_eq!(body["personParticulars"]["fullName"], "Ada Lovelace");
    assert_eq!(body["personParticulars"]["identityNumber"], Value::Null);
    assert!(!body["payslipTemplateVersion"].as_str().unwrap().is_empty());
}

fn set_employer_particulars_request(
    employer_id: &str,
    cookie: &str,
    registered_name: &str,
    acknowledged_diverging_periods: &[Value],
    reason: &str,
) -> Request<Body> {
    Request::builder()
        .method("PUT")
        .uri(format!("/api/employers/{employer_id}/particulars"))
        .header(header::COOKIE, cookie)
        .header("x-salt-request", "1")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            serde_json::json!({
                "registeredName": registered_name,
                "addressLine1": "1 Independence Ave",
                "city": "Windhoek",
                "acknowledgedDivergingPeriods": acknowledged_diverging_periods,
                "reason": reason,
            })
            .to_string(),
        ))
        .unwrap()
}

/// D22 end to end, over HTTP: what a `FinalizedPayroll` read exposes is
/// what was on record when it finalized, never what a later correction made
/// true.
#[tokio::test]
async fn a_finalized_payroll_exposes_the_particulars_frozen_at_finalize_time_not_later_corrections()
{
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;

    let response = router()
        .await
        .oneshot(set_employer_particulars_request(
            &employer_id,
            &cookie,
            "Acme Corp (Pty) Ltd",
            &[],
            "",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let finalized_payroll_id =
        finalize_a_fully_declared_employment(&employer_id, &employment_id, &cookie).await;

    let response = router()
        .await
        .oneshot(detail_request(&employer_id, &finalized_payroll_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let frozen = body_json(response).await;
    assert_eq!(
        frozen["employerParticulars"]["registeredName"],
        "Acme Corp (Pty) Ltd"
    );

    // Correcting the Employer's particulars after finalization must leave
    // the already-frozen row untouched.
    let response = router()
        .await
        .oneshot(set_employer_particulars_request(
            &employer_id,
            &cookie,
            "Acme Holdings",
            &[january_period()],
            "registered new legal name",
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = router()
        .await
        .oneshot(detail_request(&employer_id, &finalized_payroll_id, &cookie))
        .await
        .unwrap();
    let re_read = body_json(response).await;
    assert_eq!(
        re_read["employerParticulars"]["registeredName"], "Acme Corp (Pty) Ltd",
        "the frozen row must not see the later correction"
    );
}

#[tokio::test]
async fn reading_a_finalized_payrolls_traces_returns_the_paye_and_ssc_workings() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let finalized_payroll_id =
        finalize_a_fully_declared_employment(&employer_id, &employment_id, &cookie).await;

    let response = router()
        .await
        .oneshot(traces_request(&employer_id, &finalized_payroll_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;

    let paye = &body["paye"];
    assert!(paye["priorTaxableRemunerationCents"].is_i64());
    assert!(paye["priorPayeCents"].is_i64());
    assert_eq!(paye["thisPeriodTaxableRemunerationCents"], 1_500_000);
    assert!(paye["yearToDateTaxableRemunerationCents"].is_i64());
    assert!(paye["yearToDateTaxOwed"].is_string());
    assert!(paye["bandsApplied"].is_array());
    assert!(paye["periodsElapsed"].is_u64());

    for trace_name in ["employeeSsc", "employerSsc"] {
        let trace = &body[trace_name];
        assert_eq!(trace["basicPayCents"], 1_500_000);
        assert!(trace["baseCents"].is_i64());
        assert!(
            matches!(
                trace["clamp"].as_str().unwrap(),
                "none" | "floor" | "ceiling"
            ),
            "{trace_name}.clamp must be one of SscClamp's three wire strings"
        );
        assert!(trace["rate"].is_string(), "a Decimal must never be a float");
        assert!(trace["floorCents"].is_i64());
        assert!(trace["ceilingCents"].is_i64());
    }

    // A salary-only payroll carries an empty overtime list, never a
    // fabricated line (issue #76).
    assert_eq!(body["overtime"], serde_json::json!([]));

    // Never the raw snapshot (§0.29) — the acceptance criterion is "on
    // either route", so the traces response is held to it as firmly as the
    // detail response is. Only the four hand-written trace DTOs appear.
    assert!(body.get("payrollInputJson").is_none());
    assert!(body.get("payrollRulesJson").is_none());
    assert!(body.get("payrollCalculationJson").is_none());
    assert!(body.get("earningLines").is_none());
    assert!(body.get("deductions").is_none());
    let keys: Vec<&String> = body.as_object().unwrap().keys().collect();
    assert_eq!(
        keys,
        // `serde_json`'s object map is sorted, so this is the response's
        // four keys in name order, not in declaration order.
        vec!["employeeSsc", "employerSsc", "overtime", "paye"]
    );
}

#[tokio::test]
async fn a_finalized_payroll_id_belonging_to_another_employer_is_not_found_on_both_routes() {
    let (_owning_email, owning_cookie, owning_employer) = an_authorized_operator().await;
    let (_other_email, other_cookie, other_employer) = an_authorized_operator().await;
    let employment_id = create_employment(&owning_employer, &owning_cookie, "Ada Lovelace").await;
    let finalized_payroll_id =
        finalize_a_fully_declared_employment(&owning_employer, &employment_id, &owning_cookie)
            .await;

    let response = router()
        .await
        .oneshot(detail_request(
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

    let response = router()
        .await
        .oneshot(traces_request(
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
async fn an_unknown_finalized_payroll_id_is_not_found_on_both_routes() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let unknown_id = uuid::Uuid::new_v4().to_string();

    let response = router()
        .await
        .oneshot(detail_request(&employer_id, &unknown_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "finalized_payroll_not_found"
    );

    let response = router()
        .await
        .oneshot(traces_request(&employer_id, &unknown_id, &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(response).await["error"]["code"],
        "finalized_payroll_not_found"
    );
}

/// A malformed (not even a UUID) id must read the same as an unknown one
/// (404), never a 500 from a failed `::uuid` cast.
#[tokio::test]
async fn a_malformed_finalized_payroll_id_is_not_found_rather_than_a_server_error() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;

    let response = router()
        .await
        .oneshot(detail_request(&employer_id, "not-a-uuid", &cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_payroll_operator_reaches_both_routes() {
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
    let finalized_payroll_id =
        finalize_a_fully_declared_employment(&employer_id, &employment_id, &cookie).await;

    for request in [
        detail_request(&employer_id, &finalized_payroll_id, &cookie),
        traces_request(&employer_id, &finalized_payroll_id, &cookie),
    ] {
        let uri = request.uri().clone();
        let response = router().await.oneshot(request).await.unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{uri} must be reachable by a PayrollOperator"
        );
    }
}

#[tokio::test]
async fn every_route_answers_401_without_a_session() {
    let (_email, cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let finalized_payroll_id =
        finalize_a_fully_declared_employment(&employer_id, &employment_id, &cookie).await;

    for request in [
        Request::builder()
            .method("GET")
            .uri(format!(
                "/api/employers/{employer_id}/finalized-payroll/{finalized_payroll_id}"
            ))
            .body(Body::empty())
            .unwrap(),
        Request::builder()
            .method("GET")
            .uri(format!(
                "/api/employers/{employer_id}/finalized-payroll/{finalized_payroll_id}/traces"
            ))
            .body(Body::empty())
            .unwrap(),
    ] {
        let response = router().await.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn the_same_finalized_payroll_reads_back_identically_after_signing_out_and_signing_back_in() {
    let (email, cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie, "Ada Lovelace").await;
    let finalized_payroll_id =
        finalize_a_fully_declared_employment(&employer_id, &employment_id, &cookie).await;

    let first_detail = body_json(
        router()
            .await
            .oneshot(detail_request(&employer_id, &finalized_payroll_id, &cookie))
            .await
            .unwrap(),
    )
    .await;
    let first_traces = body_json(
        router()
            .await
            .oneshot(traces_request(&employer_id, &finalized_payroll_id, &cookie))
            .await
            .unwrap(),
    )
    .await;

    let logout_response = router()
        .await
        .oneshot(logout_request(&cookie))
        .await
        .unwrap();
    assert_eq!(logout_response.status(), StatusCode::OK);
    let new_cookie = login(&email).await;

    let second_detail = body_json(
        router()
            .await
            .oneshot(detail_request(
                &employer_id,
                &finalized_payroll_id,
                &new_cookie,
            ))
            .await
            .unwrap(),
    )
    .await;
    let second_traces = body_json(
        router()
            .await
            .oneshot(traces_request(
                &employer_id,
                &finalized_payroll_id,
                &new_cookie,
            ))
            .await
            .unwrap(),
    )
    .await;

    assert_eq!(first_detail, second_detail);
    assert_eq!(first_traces, second_traces);
}
