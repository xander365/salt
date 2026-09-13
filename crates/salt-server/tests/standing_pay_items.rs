//! Proves the three routes issue #79 adds: `GET` and `POST
//! /api/employers/{e}/employments/{em}/standing-pay-items` and `POST
//! .../standing-pay-items/{s}/end`, and that a run created over HTTP proposes
//! what they record. Same `oneshot`-against-the-real-router discipline as
//! `tests/employment_facts.rs`.

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

fn request(method: &str, uri: String, cookie: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-salt-request", "1");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    match body {
        Some(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

fn items_uri(employer_id: &str, employment_id: &str) -> String {
    format!("/api/employers/{employer_id}/employments/{employment_id}/standing-pay-items")
}

fn end_uri(employer_id: &str, employment_id: &str, item_id: &str) -> String {
    format!("{}/{item_id}/end", items_uri(employer_id, employment_id))
}

async fn send(request: Request<Body>) -> (StatusCode, Value) {
    let response = router().await.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, json)
}

fn allowance_body() -> Value {
    serde_json::json!({
        "kind": "taxableAllowance",
        "effectiveFrom": "2026-04-01",
        "amountCents": 50_000,
        "label": "standby",
    })
}

fn premium_body() -> Value {
    serde_json::json!({
        "kind": "medicalAidPremium",
        "effectiveFrom": "2026-04-01",
        "amountCents": 75_000,
    })
}

async fn create_item(employer_id: &str, employment_id: &str, cookie: &str, body: Value) -> String {
    let (status, json) = send(request(
        "POST",
        items_uri(employer_id, employment_id),
        Some(cookie),
        Some(body),
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    json["standingPayItemId"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn an_allowance_and_a_premium_are_recorded_and_listed_with_their_effective_from() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;

    let allowance_id = create_item(&employer_id, &employment_id, &cookie, allowance_body()).await;
    let premium_id = create_item(&employer_id, &employment_id, &cookie, premium_body()).await;

    let (status, json) = send(request(
        "GET",
        items_uri(&employer_id, &employment_id),
        Some(&cookie),
        None,
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    let items = json["standingPayItems"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["standingPayItemId"], allowance_id);
    assert_eq!(items[0]["kind"], "taxableAllowance");
    assert_eq!(items[0]["amountCents"], 50_000);
    assert_eq!(items[0]["label"], "standby");
    assert_eq!(items[0]["effectiveFrom"], "2026-04-01");
    assert_eq!(items[0]["ended"], Value::Null);
    assert!(items[0]["createdBy"].is_string());
    assert_eq!(items[1]["standingPayItemId"], premium_id);
    assert_eq!(items[1]["kind"], "medicalAidPremium");
    assert_eq!(items[1]["amountCents"], 75_000);
    assert_eq!(items[1]["ended"], Value::Null);
}

#[tokio::test]
async fn an_effective_from_that_is_not_a_period_start_is_refused_with_the_existing_message() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;
    let mut body = allowance_body();
    body["effectiveFrom"] = "2026-04-15".into();

    let (status, json) = send(request(
        "POST",
        items_uri(&employer_id, &employment_id),
        Some(&cookie),
        Some(body),
    ))
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(json["error"]["code"], "effective_from_not_a_period_start");
    assert_eq!(
        json["error"]["details"]["nextValidEffectiveFrom"],
        "2026-05-01"
    );
}

#[tokio::test]
async fn an_unlabelled_allowance_overtime_or_negative_amount_is_malformed() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;

    let mut unlabelled = allowance_body();
    unlabelled.as_object_mut().unwrap().remove("label");
    let mut blank_label = allowance_body();
    blank_label["label"] = "   ".into();
    let mut negative = premium_body();
    negative["amountCents"] = (-1).into();
    let overtime = serde_json::json!({
        "kind": "overtime",
        "effectiveFrom": "2026-04-01",
        "hours": "10",
        "multiplier": "1.5",
    });

    for body in [unlabelled, blank_label, negative, overtime] {
        let (status, json) = send(request(
            "POST",
            items_uri(&employer_id, &employment_id),
            Some(&cookie),
            Some(body.clone()),
        ))
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(json["error"]["code"], "malformed_request");
    }
}

#[tokio::test]
async fn a_zero_premium_is_refused_by_name() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;
    let mut body = premium_body();
    body["amountCents"] = 0.into();

    let (status, json) = send(request(
        "POST",
        items_uri(&employer_id, &employment_id),
        Some(&cookie),
        Some(body),
    ))
    .await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        json["error"]["code"],
        "standing_medical_aid_premium_is_zero"
    );
}

#[tokio::test]
async fn ending_an_item_states_a_reason_and_keeps_it_listed() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;
    let item_id = create_item(&employer_id, &employment_id, &cookie, allowance_body()).await;
    let uri = end_uri(&employer_id, &employment_id, &item_id);

    let (status, json) = send(request(
        "POST",
        uri.clone(),
        Some(&cookie),
        Some(serde_json::json!({ "reason": "  " })),
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        json["error"]["code"],
        "standing_pay_item_end_reason_cannot_be_empty"
    );
    let (status, json) = send(request(
        "POST",
        uri.clone(),
        Some(&cookie),
        Some(serde_json::json!({})),
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        json["error"]["code"],
        "standing_pay_item_end_reason_cannot_be_empty"
    );

    let (status, _) = send(request(
        "POST",
        uri.clone(),
        Some(&cookie),
        Some(serde_json::json!({ "reason": "opted out" })),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);

    let (_, json) = send(request(
        "GET",
        items_uri(&employer_id, &employment_id),
        Some(&cookie),
        None,
    ))
    .await;
    let items = json["standingPayItems"].as_array().unwrap();
    assert_eq!(items.len(), 1, "ending deletes nothing");
    assert_eq!(items[0]["ended"]["reason"], "opted out");
    assert!(items[0]["ended"]["endedAt"].is_string());
    assert!(items[0]["ended"]["endedBy"].is_string());

    let (status, json) = send(request(
        "POST",
        uri,
        Some(&cookie),
        Some(serde_json::json!({ "reason": "again" })),
    ))
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(json["error"]["code"], "standing_pay_item_already_ended");
}

#[tokio::test]
async fn ending_an_unknown_or_foreign_item_is_404() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;
    let other_employment_id = create_employment(&employer_id, &cookie).await;
    let item_id = create_item(&employer_id, &employment_id, &cookie, allowance_body()).await;
    let reason = serde_json::json!({ "reason": "a reason" });

    for (employment, item) in [
        (employment_id.as_str(), "not-a-uuid"),
        (
            employment_id.as_str(),
            "7d4f6a3e-0000-4000-8000-000000000001",
        ),
        (other_employment_id.as_str(), item_id.as_str()),
    ] {
        let (status, json) = send(request(
            "POST",
            end_uri(&employer_id, employment, item),
            Some(&cookie),
            Some(reason.clone()),
        ))
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{employment} {item}");
        assert_eq!(json["error"]["code"], "standing_pay_item_not_found");
    }
}

#[tokio::test]
async fn every_route_answers_404_for_an_employment_belonging_to_another_employer() {
    let (owning_cookie, owning_employer) = an_authorized_operator().await;
    let (other_cookie, other_employer) = an_authorized_operator().await;
    let employment_id = create_employment(&owning_employer, &owning_cookie).await;
    let item_id = create_item(
        &owning_employer,
        &employment_id,
        &owning_cookie,
        allowance_body(),
    )
    .await;

    for (method, uri, body) in [
        ("GET", items_uri(&other_employer, &employment_id), None),
        (
            "POST",
            items_uri(&other_employer, &employment_id),
            Some(allowance_body()),
        ),
        (
            "POST",
            end_uri(&other_employer, &employment_id, &item_id),
            Some(serde_json::json!({ "reason": "a reason" })),
        ),
    ] {
        let (status, json) = send(request(method, uri.clone(), Some(&other_cookie), body)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
        assert_eq!(json["error"]["code"], "employment_not_found");
    }
}

#[tokio::test]
async fn every_route_answers_401_without_a_session() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;
    let item_id = create_item(&employer_id, &employment_id, &cookie, allowance_body()).await;

    for (method, uri, body) in [
        ("GET", items_uri(&employer_id, &employment_id), None),
        (
            "POST",
            items_uri(&employer_id, &employment_id),
            Some(allowance_body()),
        ),
        (
            "POST",
            end_uri(&employer_id, &employment_id, &item_id),
            Some(serde_json::json!({ "reason": "a reason" })),
        ),
    ] {
        let (status, _) = send(request(method, uri.clone(), None, body)).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
    }
}

/// The whole story over HTTP: record, create the run, read it back. A
/// second identical create is refused and leaves the first run's one line.
#[tokio::test]
async fn a_run_created_over_http_proposes_the_items_once_and_says_since_when() {
    let (cookie, employer_id) = an_authorized_operator().await;
    let employment_id = create_employment(&employer_id, &cookie).await;
    let allowance_id = create_item(&employer_id, &employment_id, &cookie, allowance_body()).await;
    let premium_id = create_item(&employer_id, &employment_id, &cookie, premium_body()).await;
    let create_run = || {
        request(
            "POST",
            format!("/api/employers/{employer_id}/payroll-runs"),
            Some(&cookie),
            Some(serde_json::json!({
                "period": { "start": "2026-04-01", "end": "2026-04-30" },
                "payDate": "2026-04-30",
            })),
        )
    };

    let (status, json) = send(create_run()).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let run_id = json["payrollRunId"].as_str().unwrap().to_string();
    let (second_status, _) = send(create_run()).await;
    assert_ne!(
        second_status,
        StatusCode::OK,
        "a second run for one period is refused"
    );

    let (status, detail) = send(request(
        "GET",
        format!("/api/employers/{employer_id}/payroll-runs/{run_id}"),
        Some(&cookie),
        None,
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    let member = &detail["members"][0];
    assert_eq!(
        member["earnings"],
        serde_json::json!([{
            "kind": "taxableAllowance",
            "amountCents": 50_000,
            "label": "standby",
            "source": "standing",
            "standingPayItemId": allowance_id,
            "standingEffectiveFrom": "2026-04-01",
        }])
    );
    assert_eq!(
        member["deductions"],
        serde_json::json!([{
            "kind": "medicalAidPremium",
            "amountCents": 75_000,
            "source": "standing",
            "standingPayItemId": premium_id,
            "standingEffectiveFrom": "2026-04-01",
        }])
    );
    assert_eq!(member["basicPayProrated"], false);
}
