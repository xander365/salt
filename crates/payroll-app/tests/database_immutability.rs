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
        "person",
        "employment",
        "compensation_terms",
        "opening_balance",
        "prior_employment_declaration",
        "unsupported_deduction_declaration",
        "payroll_run",
        "payroll_run_employment",
        "payroll_run_pay_line",
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
        "INSERT INTO employer (id, name, period_end_day_kind, period_end_day_value, created_by)
         VALUES ('employer-1', 'Employer', 'day', 25, 'test-actor')",
    )
    .execute(&mut *conn)
    .await
    .expect("insert employer");

    sqlx::query(
        "INSERT INTO person (id, employer_id, full_name, created_by)
         VALUES ('person-1', 'employer-1', 'Test Person', 'test-actor')",
    )
    .execute(&mut *conn)
    .await
    .expect("insert person");

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
        "INSERT INTO payroll_run_employment (payroll_run_id, employment_id)
         VALUES ($1::uuid, 'employment-1')",
    )
    .bind(&run_id)
    .execute(&mut *conn)
    .await
    .expect("insert run membership");

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

/// The whole grant matrix, asserted table by table. `GRANT ... ON ALL TABLES`
/// covers exactly the tables that exist when it runs, so a table added by a
/// later migration reaches the restricted role with no grant at all — the
/// application would fail at run time on a table nobody thought about. Naming
/// every table here turns that omission into a failing build instead.
#[sqlx::test]
async fn the_restricted_role_holds_exactly_the_permissions_the_design_intends(pool: PgPool) {
    // Immutable history and the append-only audit trail. INV-004 (§6.2) is a
    // permission on the first; the other two record acts that happened and so
    // can never be revised or erased either.
    let append_only = [
        "finalized_payroll",
        "reversal",
        "action_log_entry",
        "person",
    ];
    // Master data, working run state, and liveness. Liveness needs DELETE
    // because a reversal deletes the row (§6.2).
    let mutable = [
        "employer",
        "employer_particulars",
        "person_particulars",
        "employment",
        "compensation_terms",
        "opening_balance",
        "prior_employment_declaration",
        "unsupported_deduction_declaration",
        // A session is lazily deleted by the lookup that finds it expired
        // (issue #44), so DELETE is exactly what its own lifecycle needs —
        // unlike an Operator or an EmployerMembership, nothing here should
        // ever be recoverable once its timers have passed.
        "session",
        "payroll_run",
        "payroll_run_employment",
        "payroll_run_pay_line",
        "working_payroll_calculation",
        "live_finalized_payroll",
    ];
    // Revisable but never erasable. An Operator is disabled by `status`
    // (issue #38 §6), and the audit trail keeps naming the id of one that is
    // gone, so `UPDATE` is exactly what disabling needs and `DELETE` is the
    // one thing no use case should ever be able to do. An EmployerMembership
    // is revoked the same way (issue #43), for the same reason. A
    // StandingPayItem is ended the same way too (issue #79): a run already
    // proposed from one still names it, so a historical proposal must
    // always have something to point at.
    let no_delete = ["operator", "employer_membership", "standing_pay_item"];

    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT tablename FROM pg_tables WHERE schemaname = 'public' ORDER BY tablename",
    )
    .fetch_all(&pool)
    .await
    .expect("list tables");

    for table in &tables {
        // `_sqlx_migrations` is sqlx's own bookkeeping, not part of this design.
        if table == "_sqlx_migrations" {
            continue;
        }
        let expected: Vec<&str> = if append_only.contains(&table.as_str()) {
            vec!["INSERT", "SELECT"]
        } else if mutable.contains(&table.as_str()) {
            vec!["DELETE", "INSERT", "SELECT", "UPDATE"]
        } else if no_delete.contains(&table.as_str()) {
            vec!["INSERT", "SELECT", "UPDATE"]
        } else {
            panic!(
                "table {table} is not named by this test, so nobody has decided what the \
                 restricted role may do to it"
            );
        };

        let granted: Vec<String> = sqlx::query_scalar(
            "SELECT privilege_type FROM information_schema.role_table_grants
             WHERE grantee = 'payroll_app' AND table_schema = 'public' AND table_name = $1
             ORDER BY privilege_type",
        )
        .bind(table)
        .fetch_all(&pool)
        .await
        .expect("read grants");

        assert_eq!(
            granted, expected,
            "the restricted role's permissions on {table} are not what the design intends"
        );
    }

    for table in append_only
        .iter()
        .chain(mutable.iter())
        .chain(no_delete.iter())
    {
        assert!(
            tables.iter().any(|t| t == table),
            "this test names {table}, but the migrations do not create it"
        );
    }
}

/// The schema's central table has no `UPDATE` grant, so an in-place JSON
/// migration is not merely discouraged but impossible (§9). A down migration
/// would be a promise to undo what the database will not let anyone undo, so
/// migrations here are forward-only and the directory must contain no reverse
/// half.
#[test]
fn there_are_no_down_migrations() {
    let migrations = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");

    let mut forward = Vec::new();
    for entry in std::fs::read_dir(&migrations).expect("read the migrations directory") {
        let name = entry.expect("read a migration entry").file_name();
        let name = name.to_string_lossy().into_owned();
        assert!(
            !name.ends_with(".down.sql"),
            "migrations are forward-only, but {name} is a down migration"
        );
        if name.ends_with(".sql") {
            forward.push(name);
        }
    }

    assert!(
        !forward.is_empty(),
        "expected the migrations directory to hold the schema, found nothing in {}",
        migrations.display()
    );
}

/// An Operator is disabled, never deleted (issue #38 §6): a removed row would
/// take an id out of the world that the audit trail still names, and ADR-0019
/// already refuses to rewrite history to tidy a schema. Asserted as a
/// permission the restricted role does not hold, so a later use case cannot
/// delete one by forgetting the rule.
#[sqlx::test]
async fn the_restricted_role_can_disable_an_operator_but_not_delete_one(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");

    sqlx::query("SET ROLE payroll_app")
        .execute(&mut *conn)
        .await
        .expect("switch to the restricted role");

    sqlx::query(
        "INSERT INTO operator (id, email, display_name, password_verifier)
         VALUES (gen_random_uuid(), 'alice@example.com', 'Alice', '$argon2id$not-a-real-hash')",
    )
    .execute(&mut *conn)
    .await
    .expect("the restricted role can create an Operator");

    sqlx::query("UPDATE operator SET status = 'disabled' WHERE email = 'alice@example.com'")
        .execute(&mut *conn)
        .await
        .expect("the restricted role can disable an Operator");

    let err = sqlx::query("DELETE FROM operator WHERE email = 'alice@example.com'")
        .execute(&mut *conn)
        .await
        .expect_err("the restricted role cannot delete an Operator");
    assert!(
        is_insufficient_privilege(&err),
        "expected an insufficient_privilege refusal while attempting to delete an Operator,          got {err:?}"
    );
}

/// `person` is append-only overall (`the_restricted_role_holds_exactly_the_permissions_the_design_intends`
/// still finds only `INSERT, SELECT` at the table level — a column grant does
/// not roll up into one), but issue #72's migration 0033 restores `UPDATE` on
/// its `full_name` column alone. Proved directly: the restricted role can
/// correct a name, but not the columns ADR-0020's scoping and §10's own
/// attribution depend on.
#[sqlx::test]
async fn the_restricted_role_can_update_only_person_full_name(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");

    sqlx::query(
        "INSERT INTO employer (id, name, period_end_day_kind, period_end_day_value, created_by)
         VALUES ('employer-1', 'Employer', 'day', 25, 'test-actor')",
    )
    .execute(&mut *conn)
    .await
    .expect("insert employer");
    sqlx::query(
        "INSERT INTO person (id, employer_id, full_name, created_by)
         VALUES ('person-1', 'employer-1', 'Misspelled Nmae', 'test-actor')",
    )
    .execute(&mut *conn)
    .await
    .expect("insert person");

    sqlx::query("SET ROLE payroll_app")
        .execute(&mut *conn)
        .await
        .expect("switch to the restricted role");

    sqlx::query("UPDATE person SET full_name = 'Corrected Name' WHERE id = 'person-1'")
        .execute(&mut *conn)
        .await
        .expect("the restricted role can correct full_name");

    let err = sqlx::query("UPDATE person SET created_by = 'someone-else' WHERE id = 'person-1'")
        .execute(&mut *conn)
        .await
        .expect_err("the restricted role cannot rewrite created_by");
    assert!(
        is_insufficient_privilege(&err),
        "expected an insufficient_privilege refusal while updating created_by, got {err:?}"
    );

    let err = sqlx::query("UPDATE person SET employer_id = 'employer-1' WHERE id = 'person-1'")
        .execute(&mut *conn)
        .await
        .expect_err("the restricted role cannot rewrite employer_id, even to its own value");
    assert!(
        is_insufficient_privilege(&err),
        "expected an insufficient_privilege refusal while updating employer_id, got {err:?}"
    );
}
