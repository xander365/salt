//! Issue #58's HTTP-to-ActionLog proof (ADR-0019). It lives in the crate
//! that owns the schema so SQL remains outside salt-server (ADR-0018).
//! The existing SQLx harness supplies a migrated, disposable database; the
//! real router handles sign-in and every payroll command below. SQL only
//! observes committed ActionLog entries, never builds a precondition.

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

async fn body_json(response: axum::response::Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Every `action_log_entry` row, whatever its Employer or actor, as
/// `(id, actor, action_type)`. The id is read as text because `sqlx` here
/// carries no `uuid` feature, and this test only ever compares ids.
async fn all_action_log_entries(pool: &PgPool) -> Vec<(String, String, String)> {
    sqlx::query_as("SELECT id::text, actor, action_type FROM action_log_entry")
        .fetch_all(pool)
        .await
        .unwrap()
}

/// Acceptance (#58): an ActionLog entry written through an HTTP call names
/// the session's Operator, and a body claiming a different actor changes
/// nothing about it. Two differently shaped commands are driven — one on an
/// Employment, one on a PayrollRun — so the proof is about the router's own
/// rule (§0.22: the actor is `operator:<OperatorId>` from
/// `AuthorizedEmployerContext`, never a body field) rather than about one
/// handler that happens to be written correctly.
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

    // Whatever the setup above already logged is not this test's subject.
    let before: Vec<String> = all_action_log_entries(&pool)
        .await
        .into_iter()
        .map(|(id, _, _)| id)
        .collect();

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
    let employer_id = owner.employer_id.to_string();

    let created = router
        .clone()
        .oneshot(post_json(
            &format!("/api/employers/{employer_id}/employments"),
            Some(&cookie),
            json!({ "fullName": "Ada Lovelace", "startDate": "2026-01-01" }),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let employment_id = body_json(created).await["employmentId"]
        .as_str()
        .unwrap()
        .to_string();

    // A declaration on an Employment, forging an actor: `ActionType::
    // PriorEmploymentDeclared` on target_type `employment`.
    let declared = router
        .clone()
        .oneshot(post_json(
            &format!("/api/employers/{employer_id}/employments/{employment_id}/prior-employment"),
            Some(&cookie),
            json!({ "taxYear": 2025, "status": "confirmed_none", "actor": forged_actor }),
        ))
        .await
        .unwrap();
    assert_eq!(declared.status(), StatusCode::OK);

    // Two PayrollRun creations, identical but for the forged `actor`.
    // Separate valid periods avoid the one-Ordinary-run-per-period guard.
    // Both must record Alice, not merely agree with one another.
    let mut run_ids = Vec::new();
    let mut run_bodies = Vec::new();
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
                &format!("/api/employers/{employer_id}/payroll-runs"),
                Some(&cookie),
                body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = body_json(response).await;
        run_ids.push(response["payrollRunId"].as_str().unwrap().to_string());
        run_bodies.push(response);
    }

    // The forged body changed nothing HTTP-observable: same keys, same
    // shape, differing only where the request itself differed.
    let keys = |body: &Value| {
        let mut keys: Vec<String> = body.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        keys
    };
    assert_eq!(keys(&run_bodies[0]), keys(&run_bodies[1]));

    let written: Vec<(String, String, String)> = all_action_log_entries(&pool)
        .await
        .into_iter()
        .filter(|(id, _, _)| !before.contains(id))
        .collect();

    // The HTTP calls above really did write the log this test is about.
    let mut action_types: Vec<&str> = written
        .iter()
        .map(|(_, _, action_type)| action_type.as_str())
        .collect();
    action_types.sort_unstable();
    assert_eq!(
        action_types,
        [
            "payroll_run_created",
            "payroll_run_created",
            "prior_employment_declared"
        ],
        "the four HTTP calls above must have written exactly these ActionLog entries",
    );

    // Every one of them names the session's Operator.
    for (id, actor, action_type) in &written {
        assert_eq!(
            actor, &expected_actor,
            "ActionLog entry {id} ({action_type}) must name the session's Operator",
        );
    }

    // And the forged actor reached nothing at all, in any Employer.
    let forged_rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM action_log_entry WHERE actor = $1")
            .bind(&forged_actor)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        forged_rows, 0,
        "a body's `actor` field must never reach action_log_entry.actor",
    );

    // Belt and braces on the run the forged body created: exactly one entry,
    // naming Alice, under the id the response handed back.
    let actors: Vec<String> = sqlx::query_scalar(
        "SELECT actor FROM action_log_entry
         WHERE employer_id = $1 AND target_type = 'payroll_run'
           AND target_id = $2 AND action_type = 'payroll_run_created'",
    )
    .bind(employer_id.as_str())
    .bind(&run_ids[1])
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(actors, vec![expected_actor]);
}
