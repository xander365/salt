//! Issue #58's HTTP-to-ActionLog proof (ADR-0019). It lives in the crate
//! that owns the schema so SQL remains outside salt-server (ADR-0018).
//! The existing SQLx harness supplies a migrated, disposable database; the
//! real router handles sign-in and both payroll commands. SQL only observes
//! committed ActionLog entries, never builds a precondition.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use payroll_app::{BootstrapPeriodEndDay, SaltDatabase};
use salt_server::{AppState, build_router};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

fn post_json(uri: &str, cookie: Option<&str>, body: Value) -> Request<Body> {
    let mut request = Request::builder()
        .method("POST")
        .uri(uri)
        .header("x-salt-request", "1")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(cookie) = cookie {
        request = request.header(header::COOKIE, cookie);
    }
    request.body(Body::from(body.to_string())).unwrap()
}

#[sqlx::test]
async fn http_action_log_names_the_session_operator_even_when_the_body_forges_an_actor(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool.clone());
    let password = "correct horse battery staple";
    // Bootstrap and further Operators have no HTTP routes (§0.2, §0.39).
    let owner = payroll_app::bootstrap(
        &db,
        "alice@example.com",
        "Alice",
        password,
        "Acme Corp",
        BootstrapPeriodEndDay::LastDayOfMonth,
    )
    .await
    .unwrap();
    let other_operator =
        payroll_app::create_operator(&db, "mallory@example.com", "Mallory", password)
            .await
            .unwrap();
    let expected_actor = format!("operator:{}", owner.operator_id);
    let forged_actor = format!("operator:{other_operator}");
    assert_ne!(expected_actor, forged_actor);

    let router = build_router(AppState::new(db, true));
    let login = router
        .clone()
        .oneshot(post_json(
            "/api/session",
            None,
            json!({ "email": "alice@example.com", "password": password }),
        ))
        .await
        .unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let cookie = login.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    // Separate valid periods avoid the one-Ordinary-run-per-period guard.
    // Both commands must record Alice, not merely agree with one another.
    for body in [
        json!({
            "period": { "start": "2026-01-01", "end": "2026-01-31" },
            "payDate": "2026-02-05",
        }),
        json!({
            "period": { "start": "2026-02-01", "end": "2026-02-28" },
            "payDate": "2026-03-05",
            "actor": forged_actor,
        }),
    ] {
        let response = router
            .clone()
            .oneshot(post_json(
                &format!("/api/employers/{}/payroll-runs", owner.employer_id),
                Some(&cookie),
                body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let response: Value = serde_json::from_slice(&bytes).unwrap();
        let run_id = response["payrollRunId"].as_str().unwrap();

        let actors: Vec<String> = sqlx::query_scalar(
            "SELECT actor FROM action_log_entry
             WHERE employer_id = $1 AND target_type = 'payroll_run'
               AND target_id = $2 AND action_type = 'payroll_run_created'",
        )
        .bind(owner.employer_id.as_str())
        .bind(run_id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            actors,
            vec![expected_actor.clone()],
            "HTTP-created run {run_id} must have exactly one ActionLog entry naming the session's Operator",
        );
    }
}
