//! Proves INV-004 is a database permission, not application discipline
//! (docs/domain/payroll-run-persistence.md §6.2): a connection operating as
//! the restricted `payroll_app` role is refused an `UPDATE` and a `DELETE`
//! on `finalized_payroll`. A test connecting as the migration-applying
//! (owner) role would pass while proving nothing, so every assertion here
//! runs after `SET ROLE payroll_app`.

use sqlx::{PgPool, Row};

const INSUFFICIENT_PRIVILEGE: &str = "42501";

fn is_insufficient_privilege(err: &sqlx::Error) -> bool {
    matches!(err, sqlx::Error::Database(db_err) if db_err.code().as_deref() == Some(INSUFFICIENT_PRIVILEGE))
}

#[sqlx::test]
async fn migrations_create_the_schema_this_design_names(pool: PgPool) {
    let tables: Vec<String> = sqlx::query(
        "SELECT tablename FROM pg_tables WHERE schemaname = 'public' ORDER BY tablename",
    )
    .fetch_all(&pool)
    .await
    .expect("list tables")
    .into_iter()
    .map(|row| row.get::<String, _>("tablename"))
    .collect();

    for expected in [
        "employer",
        "employment",
        "compensation_terms",
        "opening_balance",
        "prior_employment_declaration",
        "unsupported_deduction_declaration",
        "payroll_run",
        "payroll_run_employment",
        "payroll_run_earning",
        "working_payroll_calculation",
        "finalized_payroll",
        "live_finalized_payroll",
        "reversal",
        "action_log_entry",
    ] {
        assert!(
            tables.iter().any(|t| t == expected),
            "expected migrations to create table {expected}, found {tables:?}"
        );
    }
}

/// Inserts one employer, one employment, and one finalized payroll, and
/// returns the finalized payroll's id (as text — the id column is a native
/// `uuid`, but casting to text keeps this fixture free of an extra crate
/// dependency on `uuid`).
async fn a_finalized_payroll(conn: &mut sqlx::PgConnection) -> String {
    sqlx::query(
        "INSERT INTO employer (id, period_end_day_kind, period_end_day_value, created_by)
         VALUES ('employer-1', 'day', 25, 'test-actor')",
    )
    .execute(&mut *conn)
    .await
    .expect("insert employer");

    sqlx::query(
        "INSERT INTO employment (id, employer_id, person_id, start_date, created_by)
         VALUES ('employment-1', 'employer-1', 'person-1', '2026-03-01', 'test-actor')",
    )
    .execute(&mut *conn)
    .await
    .expect("insert employment");

    let run_id: String = sqlx::query(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status, created_by)
         VALUES
            ('employer-1', '2026-03-01', '2026-03-31', '2026-04-05', 'ordinary', 'finalized', 'test-actor')
         RETURNING id::text",
    )
    .fetch_one(&mut *conn)
    .await
    .expect("insert payroll_run")
    .get(0);

    sqlx::query(
        "INSERT INTO finalized_payroll
            (payroll_run_id, employment_id, employer_id, period_start, period_end, tax_year,
             payroll_input_json, payroll_rules_json, payroll_calculation_json,
             taxable_remuneration, paye, paye_table_id, ssc_rules_id, salt_version, finalized_by)
         VALUES
            ($1::uuid, 'employment-1', 'employer-1', '2026-03-01', '2026-03-31', 2026,
             '{}', '{}', '{}', 15000.00, 1200.00, 'paye-1', 'ssc-1', '0.1.0+gdeadbeef',
             'test-actor')
         RETURNING id::text",
    )
    .bind(&run_id)
    .fetch_one(&mut *conn)
    .await
    .expect("insert finalized_payroll")
    .get(0)
}

#[sqlx::test]
async fn the_restricted_role_cannot_update_a_finalized_payroll(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    let finalized_payroll_id = a_finalized_payroll(&mut conn).await;

    sqlx::query("SET ROLE payroll_app")
        .execute(&mut *conn)
        .await
        .expect("switch to the restricted role");

    let result = sqlx::query("UPDATE finalized_payroll SET paye = 0 WHERE id = $1::uuid")
        .bind(&finalized_payroll_id)
        .execute(&mut *conn)
        .await;

    let err = result.expect_err("the restricted role's UPDATE must be refused");
    assert!(
        is_insufficient_privilege(&err),
        "expected an insufficient_privilege refusal, got {err:?}"
    );
}

#[sqlx::test]
async fn the_restricted_role_cannot_delete_a_finalized_payroll(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    let finalized_payroll_id = a_finalized_payroll(&mut conn).await;

    sqlx::query("SET ROLE payroll_app")
        .execute(&mut *conn)
        .await
        .expect("switch to the restricted role");

    let result = sqlx::query("DELETE FROM finalized_payroll WHERE id = $1::uuid")
        .bind(&finalized_payroll_id)
        .execute(&mut *conn)
        .await;

    let err = result.expect_err("the restricted role's DELETE must be refused");
    assert!(
        is_insufficient_privilege(&err),
        "expected an insufficient_privilege refusal, got {err:?}"
    );
}

#[sqlx::test]
async fn the_restricted_role_can_still_insert_and_select(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    let finalized_payroll_id = a_finalized_payroll(&mut conn).await;

    sqlx::query("SET ROLE payroll_app")
        .execute(&mut *conn)
        .await
        .expect("switch to the restricted role");

    let row = sqlx::query("SELECT id::text FROM finalized_payroll WHERE id = $1::uuid")
        .bind(&finalized_payroll_id)
        .fetch_one(&mut *conn)
        .await
        .expect("the restricted role can still read finalized_payroll");
    assert_eq!(row.get::<String, _>(0), finalized_payroll_id);
}

#[sqlx::test]
async fn the_restricted_role_cannot_mutate_reversals_or_action_log_entries(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    let finalized_payroll_id = a_finalized_payroll(&mut conn).await;

    sqlx::query("SET ROLE payroll_app")
        .execute(&mut *conn)
        .await
        .expect("switch to the restricted role");

    sqlx::query(
        "INSERT INTO reversal (finalized_payroll_id, reversed_by, reason)
         VALUES ($1::uuid, 'test-actor', 'test reversal')",
    )
    .bind(&finalized_payroll_id)
    .execute(&mut *conn)
    .await
    .expect("the restricted role can create a reversal");

    sqlx::query(
        "INSERT INTO action_log_entry (employer_id, actor, action_type, target_type, target_id)
         VALUES ('employer-1', 'test-actor', 'payroll_finalized', 'payroll_run', 'test-run')",
    )
    .execute(&mut *conn)
    .await
    .expect("the restricted role can append an action log entry");

    for (action, result) in [
        (
            "update a reversal",
            sqlx::query(
                "UPDATE reversal SET reason = 'changed' WHERE finalized_payroll_id = $1::uuid",
            )
            .bind(&finalized_payroll_id)
            .execute(&mut *conn)
            .await,
        ),
        (
            "delete a reversal",
            sqlx::query("DELETE FROM reversal WHERE finalized_payroll_id = $1::uuid")
                .bind(&finalized_payroll_id)
                .execute(&mut *conn)
                .await,
        ),
        (
            "update an action log entry",
            sqlx::query(
                "UPDATE action_log_entry SET actor = 'changed' WHERE employer_id = 'employer-1'",
            )
            .execute(&mut *conn)
            .await,
        ),
        (
            "delete an action log entry",
            sqlx::query("DELETE FROM action_log_entry WHERE employer_id = 'employer-1'")
                .execute(&mut *conn)
                .await,
        ),
    ] {
        let err = result.expect_err(&format!("the restricted role cannot {action}"));
        assert!(
            is_insufficient_privilege(&err),
            "expected an insufficient_privilege refusal while attempting to {action}, got {err:?}"
        );
    }
}
