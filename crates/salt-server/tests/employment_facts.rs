//! Proves the four routes issue #52 adds: `POST
//! /api/employers/{e}/employments/{em}/compensation-terms`,
//! `.../prior-employment`, `.../unsupported-deductions` and
//! `.../opening-balance` (parent #49 Spec 2 of 3). Same `oneshot`-against-the-
//! real-router discipline as `tests/employments.rs`.

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
         (see crates/salt-server/tests/employment_facts.rs)",
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

fn create_employment_request(employer_id: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!("/api/employers/{employer_id}/employments"))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-salt-request", "1")
        .body(Body::from(
            serde_json::json!({
                "fullName": "Ada Lovelace",
                "startDate": "2026-03-26",
            })
            .to_string(),
        ))
        .unwrap()
}

async fn create_employment(employer_id: &str, cookie: &str) -> String {
    let response = router()
        .await
        .oneshot(create_employment_request(employer_id, cookie))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    body_json(response).await["employmentId"]
        .as_str()
        .unwrap()
        .to_string()
}

fn post_request(
    employer_id: &str,
    employment_id: &str,
    route: &str,
    cookie: &str,
    salt_header: bool,
    body: Value,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(format!(
            "/api/employers/{employer_id}/employments/{employment_id}/{route}"
        ))
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json");
    if salt_header {
        builder = builder.header("x-salt-request", "1");
    }
    builder.body(Body::from(body.to_string())).unwrap()
}

fn compensation_terms_body() -> Value {
    serde_json::json!({
        "effectiveFrom": "2026-04-01",
        "basicPayCents": 1_500_000,
        "acknowledgedDivergingPeriods": [],
        "reason": "",
    })
}

fn prior_employment_confirmed_none_body() -> Value {
    serde_json::json!({ "taxYear": 2026, "status": "confirmed_none" })
}

fn unsupported_deductions_confirmed_none_body() -> Value {
    serde_json::json!({
        "effectiveFrom": "2026-04-01",
        "status": "confirmed_none",
        "acknowledgedDivergingPeriods": [],
        "reason": "no unsupported deductions",
    })
}

fn opening_balance_body() -> Value {
    serde_json::json!({
        "taxYear": 2026,
        "saltCoverageStart": "2026-04-30",
        "priorTaxableRemunerationCents": 10_000,
        "priorPayeCents": 2_000,
    })
}

#[tokio::test]
async fn recording_compensation_terms_succeeds_and_is_reflected_on_the_detail_route() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(post_request(
            &employer_id,
            &employment_id,
            "compensation-terms",
            &cookie,
            true,
            compensation_terms_body(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["divergingPeriods"], serde_json::json!([]));

    let detail = router()
        .await
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!(
                    "/api/employers/{employer_id}/employments/{employment_id}"
                ))
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let detail = body_json(detail).await;
    assert_eq!(detail["currentBasicPayCents"], 1_500_000);
}

#[tokio::test]
async fn a_negative_basic_pay_is_a_bad_request() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;

    let mut body = compensation_terms_body();
    body["basicPayCents"] = serde_json::json!(-100);

    let response = router()
        .await
        .oneshot(post_request(
            &employer_id,
            &employment_id,
            "compensation-terms",
            &cookie,
            true,
            body,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "malformed_request");
}

#[tokio::test]
async fn an_invalid_acknowledgement_period_is_a_bad_request() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;

    let mut body = compensation_terms_body();
    body["acknowledgedDivergingPeriods"] = serde_json::json!([
        { "start": "2026-05-01", "end": "2026-04-30" }
    ]);

    let response = router()
        .await
        .oneshot(post_request(
            &employer_id,
            &employment_id,
            "compensation-terms",
            &cookie,
            true,
            body,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "malformed_request");
}

/// An effective date that is not a period start is the calculator's own
/// refusal (INV-014), mapped by issue #50's exhaustive `match` — never a
/// rule this route re-checks itself.
#[tokio::test]
async fn an_effective_from_that_is_not_a_period_start_is_the_calculators_own_refusal() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;

    let mut body = compensation_terms_body();
    body["effectiveFrom"] = serde_json::json!("2026-04-15");

    let response = router()
        .await
        .oneshot(post_request(
            &employer_id,
            &employment_id,
            "compensation-terms",
            &cookie,
            true,
            body,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "effective_from_not_a_period_start");
}

#[tokio::test]
async fn declaring_prior_employment_confirmed_none_succeeds() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(post_request(
            &employer_id,
            &employment_id,
            "prior-employment",
            &cookie,
            true,
            prior_employment_confirmed_none_body(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn declaring_prior_employment_present_carries_the_figures() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(post_request(
            &employer_id,
            &employment_id,
            "prior-employment",
            &cookie,
            true,
            serde_json::json!({
                "taxYear": 2026,
                "status": "present",
                "taxableRemunerationCents": 500_000,
                "payeCents": 75_000,
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn an_unknown_prior_employment_status_is_a_bad_request() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(post_request(
            &employer_id,
            &employment_id,
            "prior-employment",
            &cookie,
            true,
            serde_json::json!({ "taxYear": 2026, "status": "unknown" }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "malformed_request");
}

#[tokio::test]
async fn present_prior_employment_without_figures_is_a_bad_request() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(post_request(
            &employer_id,
            &employment_id,
            "prior-employment",
            &cookie,
            true,
            serde_json::json!({ "taxYear": 2026, "status": "present" }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "malformed_request");
}

#[tokio::test]
async fn declaring_unsupported_deductions_confirmed_none_succeeds() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(post_request(
            &employer_id,
            &employment_id,
            "unsupported-deductions",
            &cookie,
            true,
            unsupported_deductions_confirmed_none_body(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let json = body_json(response).await;
    assert_eq!(json["divergingPeriods"], serde_json::json!([]));
}

#[tokio::test]
async fn declaring_unsupported_deductions_present_carries_the_kinds() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(post_request(
            &employer_id,
            &employment_id,
            "unsupported-deductions",
            &cookie,
            true,
            serde_json::json!({
                "effectiveFrom": "2026-04-01",
                "status": "present",
                "kinds": ["provident_fund", "education_policy"],
                "acknowledgedDivergingPeriods": [],
                "reason": "joined a provident fund",
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn an_unknown_unsupported_deduction_kind_is_a_bad_request() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(post_request(
            &employer_id,
            &employment_id,
            "unsupported-deductions",
            &cookie,
            true,
            serde_json::json!({
                "effectiveFrom": "2026-04-01",
                "status": "present",
                "kinds": ["not-a-real-kind"],
                "acknowledgedDivergingPeriods": [],
                "reason": "",
            }),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json = body_json(response).await;
    assert_eq!(json["error"]["code"], "malformed_request");
}

#[tokio::test]
async fn recording_an_opening_balance_succeeds() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;

    let response = router()
        .await
        .oneshot(post_request(
            &employer_id,
            &employment_id,
            "opening-balance",
            &cookie,
            true,
            opening_balance_body(),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn every_route_demands_the_salt_request_header() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;

    for (route, body) in [
        ("compensation-terms", compensation_terms_body()),
        ("prior-employment", prior_employment_confirmed_none_body()),
        (
            "unsupported-deductions",
            unsupported_deductions_confirmed_none_body(),
        ),
        ("opening-balance", opening_balance_body()),
    ] {
        let response = router()
            .await
            .oneshot(post_request(
                &employer_id,
                &employment_id,
                route,
                &cookie,
                false,
                body,
            ))
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "{route} must demand X-Salt-Request"
        );
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "salt_request_header_required");
    }
}

#[tokio::test]
async fn every_route_answers_404_for_an_employment_belonging_to_another_employer() {
    let (owning_cookie, owning_employer) = an_authorized_operator().await;
    let (other_cookie, other_employer) = an_authorized_operator().await;
    let employment_id = create_employment(&owning_employer, &owning_cookie).await;

    for (route, body) in [
        ("compensation-terms", compensation_terms_body()),
        ("prior-employment", prior_employment_confirmed_none_body()),
        (
            "unsupported-deductions",
            unsupported_deductions_confirmed_none_body(),
        ),
        ("opening-balance", opening_balance_body()),
    ] {
        let response = router()
            .await
            .oneshot(post_request(
                &other_employer,
                &employment_id,
                route,
                &other_cookie,
                true,
                body,
            ))
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{route} must 404 an employment from another employer"
        );
        let json = body_json(response).await;
        assert_eq!(json["error"]["code"], "employment_not_found");
    }
}

#[tokio::test]
async fn every_route_answers_404_without_membership() {
    let (owning_cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &owning_cookie).await;
    let outsider_email = unique_email("mallory");
    create_operator(&outsider_email).await;
    let outsider_cookie = login(&outsider_email).await;

    for (route, body) in [
        ("compensation-terms", compensation_terms_body()),
        ("prior-employment", prior_employment_confirmed_none_body()),
        (
            "unsupported-deductions",
            unsupported_deductions_confirmed_none_body(),
        ),
        ("opening-balance", opening_balance_body()),
    ] {
        let response = router()
            .await
            .oneshot(post_request(
                &employer_id,
                &employment_id,
                route,
                &outsider_cookie,
                true,
                body,
            ))
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{route} must 404 a non-member"
        );
    }
}

/// A `PayrollOperator`, not only an `Owner`, is let through all four routes
/// (§0.6, issue #52's own Deep Instructions: none of these four is
/// Owner-only).
#[tokio::test]
async fn a_payroll_operator_reaches_all_four_routes() {
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
    let employment_id = create_employment(&employer_id, &cookie).await;

    for (route, body) in [
        ("compensation-terms", compensation_terms_body()),
        ("prior-employment", prior_employment_confirmed_none_body()),
        (
            "unsupported-deductions",
            unsupported_deductions_confirmed_none_body(),
        ),
        ("opening-balance", opening_balance_body()),
    ] {
        let response = router()
            .await
            .oneshot(post_request(
                &employer_id,
                &employment_id,
                route,
                &cookie,
                true,
                body,
            ))
            .await
            .unwrap();

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{route} must be reachable by a PayrollOperator"
        );
    }
}
