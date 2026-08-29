//! Proves the constraints docs/domain/payroll-run-persistence.md §11 places in
//! PostgreSQL rather than in Rust, because their absence would be a silent,
//! hard-to-notice regression rather than a failing calculation: the uniqueness
//! rules that make a contradictory second row unrepresentable, the foreign keys
//! that keep a permanent history row agreeing with the run it came from, the
//! `CHECK`s that demand a reason for a correction and for a removal, and the
//! two triggers standing in for the one rule a single-table `CHECK` cannot
//! state because it spans two tables.
//!
//! Each test states a payroll fact rather than a schema fact, and reaches it
//! through the same door a use case will: SQL against a fresh migrated
//! database. `#[sqlx::test]` builds that database per test, so nothing here
//! shares state with anything else.

use std::time::Duration;

use sqlx::{Acquire, PgPool};
use tokio::sync::oneshot;

const UNIQUE_VIOLATION: &str = "23505";

fn is_unique_violation(err: &sqlx::Error) -> bool {
    matches!(err, sqlx::Error::Database(db_err) if db_err.code().as_deref() == Some(UNIQUE_VIOLATION))
}

async fn an_employer_and_two_employments(conn: &mut sqlx::PgConnection) {
    sqlx::query(
        "INSERT INTO employer (id, period_end_day_kind, period_end_day_value, created_by)
         VALUES ('employer-1', 'day', 25, 'actor')",
    )
    .execute(&mut *conn)
    .await
    .expect("insert employer");

    sqlx::query(
        "INSERT INTO employment (id, employer_id, person_id, start_date, created_by)
         VALUES ('emp-1', 'employer-1', 'person-1', '2026-03-01', 'actor'),
                ('emp-2', 'employer-1', 'person-2', '2026-03-01', 'actor')",
    )
    .execute(&mut *conn)
    .await
    .expect("insert employments");
}

#[sqlx::test]
async fn a_correction_run_refuses_a_second_employment(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    an_employer_and_two_employments(&mut conn).await;

    let run_id: String = sqlx::query_scalar(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status,
             correction_reason, created_by)
         VALUES
            ('employer-1', '2026-03-01', '2026-03-31', '2026-06-05', 'correction', 'draft',
             'fix march', 'actor')
         RETURNING id::text",
    )
    .fetch_one(&mut *conn)
    .await
    .expect("insert correction run");

    sqlx::query(
        "INSERT INTO payroll_run_employment (payroll_run_id, employment_id)
         VALUES ($1::uuid, 'emp-1')",
    )
    .bind(&run_id)
    .execute(&mut *conn)
    .await
    .expect("the first Employment may join a Correction run");

    let second_member = sqlx::query(
        "INSERT INTO payroll_run_employment (payroll_run_id, employment_id)
         VALUES ($1::uuid, 'emp-2')",
    )
    .bind(&run_id)
    .execute(&mut *conn)
    .await;

    assert!(
        second_member.is_err(),
        "a Correction run must refuse a second Employment"
    );
}

#[sqlx::test]
async fn an_ordinary_run_permits_more_than_one_employment(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    an_employer_and_two_employments(&mut conn).await;

    let run_id: String = sqlx::query_scalar(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status, created_by)
         VALUES
            ('employer-1', '2026-03-01', '2026-03-31', '2026-04-05', 'ordinary', 'draft', 'actor')
         RETURNING id::text",
    )
    .fetch_one(&mut *conn)
    .await
    .expect("insert ordinary run");

    sqlx::query(
        "INSERT INTO payroll_run_employment (payroll_run_id, employment_id)
         VALUES ($1::uuid, 'emp-1'), ($1::uuid, 'emp-2')",
    )
    .bind(&run_id)
    .execute(&mut *conn)
    .await
    .expect("an Ordinary run auto-proposes every overlapping Employment");
}

/// §4.3: a mis-created Employment is void rather than deleted, and therefore
/// cannot be proposed into a run. The converse guard matters too: otherwise a
/// member could become void after it had already entered a run.
#[sqlx::test]
async fn a_voided_employment_cannot_be_or_become_a_run_member(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    an_employer_and_two_employments(&mut conn).await;

    let run_id: String = sqlx::query_scalar(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status, created_by)
         VALUES
            ('employer-1', '2026-03-01', '2026-03-31', '2026-04-05', 'ordinary', 'draft', 'actor')
         RETURNING id::text",
    )
    .fetch_one(&mut *conn)
    .await
    .expect("insert ordinary run");

    sqlx::query("UPDATE employment SET is_void = TRUE WHERE id = 'emp-2'")
        .execute(&mut *conn)
        .await
        .expect("void an Employment that is not a run member");

    let void_member = sqlx::query(
        "INSERT INTO payroll_run_employment (payroll_run_id, employment_id)
         VALUES ($1::uuid, 'emp-2')",
    )
    .bind(&run_id)
    .execute(&mut *conn)
    .await;
    assert!(
        void_member.is_err(),
        "a voided Employment must not enter run membership"
    );

    sqlx::query(
        "INSERT INTO payroll_run_employment (payroll_run_id, employment_id)
         VALUES ($1::uuid, 'emp-1')",
    )
    .bind(&run_id)
    .execute(&mut *conn)
    .await
    .expect("an active Employment can join a run");

    let void_member = sqlx::query("UPDATE employment SET is_void = TRUE WHERE id = 'emp-1'")
        .execute(&mut *conn)
        .await;
    assert!(void_member.is_err(), "a run member must not become void");
}

#[sqlx::test]
async fn only_one_ordinary_run_exists_per_employer_and_pay_period(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    an_employer_and_two_employments(&mut conn).await;

    sqlx::query(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status, created_by)
         VALUES
            ('employer-1', '2026-03-01', '2026-03-31', '2026-04-05', 'ordinary', 'draft', 'actor')",
    )
    .execute(&mut *conn)
    .await
    .expect("insert the first Ordinary run for March");

    let second_march_run = sqlx::query(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status, created_by)
         VALUES
            ('employer-1', '2026-03-01', '2026-03-31', '2026-04-05', 'ordinary', 'draft', 'actor')",
    )
    .execute(&mut *conn)
    .await;

    let err =
        second_march_run.expect_err("a second Ordinary run for the same period must be refused");
    assert!(
        is_unique_violation(&err),
        "expected a unique_violation, got {err:?}"
    );

    sqlx::query(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status, correction_reason, created_by)
         VALUES
            ('employer-1', '2026-03-01', '2026-03-31', '2026-06-05', 'correction', 'draft', 'fix march', 'actor')",
    )
    .execute(&mut *conn)
    .await
    .expect("Correction runs for the same period are unconstrained in number");
}

#[sqlx::test]
async fn a_reversed_finalized_payroll_can_be_replaced_at_most_once(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    an_employer_and_two_employments(&mut conn).await;

    let ordinary_run_id: String = sqlx::query_scalar(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status, created_by)
         VALUES
            ('employer-1', '2026-03-01', '2026-03-31', '2026-04-05', 'ordinary', 'finalized', 'actor')
         RETURNING id::text",
    )
    .fetch_one(&mut *conn)
    .await
    .expect("insert ordinary run");

    sqlx::query(
        "INSERT INTO payroll_run_employment (payroll_run_id, employment_id)
         VALUES ($1::uuid, 'emp-1')",
    )
    .bind(&ordinary_run_id)
    .execute(&mut *conn)
    .await
    .expect("emp-1 is a member of the Ordinary run");

    let original: String = sqlx::query_scalar(
        "INSERT INTO finalized_payroll
            (payroll_run_id, employment_id, employer_id, period_start, period_end, tax_year,
             payroll_input_json, payroll_rules_json, payroll_calculation_json,
             taxable_remuneration, paye, paye_table_id, ssc_rules_id, salt_version, finalized_by)
         VALUES
            ($1::uuid, 'emp-1', 'employer-1', '2026-03-01', '2026-03-31', 2026,
             '{}', '{}', '{}', 15000, 1200, 'paye-1', 'ssc-1', '0.1.0+gdeadbeef', 'actor')
         RETURNING id::text",
    )
    .bind(&ordinary_run_id)
    .fetch_one(&mut *conn)
    .await
    .expect("insert the original finalized payroll");

    let correction_run_id: String = sqlx::query_scalar(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status, correction_reason, created_by)
         VALUES
            ('employer-1', '2026-03-01', '2026-03-31', '2026-06-05', 'correction', 'finalized', 'fix march', 'actor')
         RETURNING id::text",
    )
    .fetch_one(&mut *conn)
    .await
    .expect("insert first correction run");

    sqlx::query(
        "INSERT INTO payroll_run_employment (payroll_run_id, employment_id)
         VALUES ($1::uuid, 'emp-1')",
    )
    .bind(&correction_run_id)
    .execute(&mut *conn)
    .await
    .expect("emp-1 is the Correction run's single member");

    sqlx::query(
        "INSERT INTO finalized_payroll
            (payroll_run_id, employment_id, employer_id, period_start, period_end, tax_year,
             replaces_finalized_payroll_id,
             payroll_input_json, payroll_rules_json, payroll_calculation_json,
             taxable_remuneration, paye, paye_table_id, ssc_rules_id, salt_version, finalized_by)
         VALUES
            ($1::uuid, 'emp-1', 'employer-1', '2026-03-01', '2026-03-31', 2026, $2::uuid,
             '{}', '{}', '{}', 16000, 1300, 'paye-1', 'ssc-1', '0.1.0+gdeadbeef', 'actor')",
    )
    .bind(&correction_run_id)
    .bind(&original)
    .execute(&mut *conn)
    .await
    .expect("the first replacement of the original is allowed");

    let another_correction_run_id: String = sqlx::query_scalar(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status, correction_reason, created_by)
         VALUES
            ('employer-1', '2026-03-01', '2026-03-31', '2026-07-05', 'correction', 'finalized', 'fix march again', 'actor')
         RETURNING id::text",
    )
    .fetch_one(&mut *conn)
    .await
    .expect("insert second correction run");

    sqlx::query(
        "INSERT INTO payroll_run_employment (payroll_run_id, employment_id)
         VALUES ($1::uuid, 'emp-1')",
    )
    .bind(&another_correction_run_id)
    .execute(&mut *conn)
    .await
    .expect("emp-1 is the second Correction run's single member");

    let second_replacement = sqlx::query(
        "INSERT INTO finalized_payroll
            (payroll_run_id, employment_id, employer_id, period_start, period_end, tax_year,
             replaces_finalized_payroll_id,
             payroll_input_json, payroll_rules_json, payroll_calculation_json,
             taxable_remuneration, paye, paye_table_id, ssc_rules_id, salt_version, finalized_by)
         VALUES
            ($1::uuid, 'emp-1', 'employer-1', '2026-03-01', '2026-03-31', 2026, $2::uuid,
             '{}', '{}', '{}', 17000, 1400, 'paye-1', 'ssc-1', '0.1.0+gdeadbeef', 'actor')",
    )
    .bind(&another_correction_run_id)
    .bind(&original)
    .execute(&mut *conn)
    .await;

    let err = second_replacement
        .expect_err("the same original finalized payroll cannot be replaced twice");
    assert!(
        is_unique_violation(&err),
        "expected a unique_violation, got {err:?}"
    );
}

#[sqlx::test]
async fn concurrent_membership_additions_leave_a_correction_run_with_one_member(pool: PgPool) {
    let mut setup_connection = pool.acquire().await.expect("acquire setup connection");
    an_employer_and_two_employments(&mut setup_connection).await;

    let run_id: String = sqlx::query_scalar(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status,
             correction_reason, created_by)
         VALUES
            ('employer-1', '2026-03-01', '2026-03-31', '2026-06-05', 'correction', 'draft',
             'fix march', 'actor')
         RETURNING id::text",
    )
    .fetch_one(&mut *setup_connection)
    .await
    .expect("insert correction run");

    let mut first_connection = pool.acquire().await.expect("acquire first connection");
    let mut first_transaction = first_connection
        .begin()
        .await
        .expect("begin first transaction");
    sqlx::query(
        "INSERT INTO payroll_run_employment (payroll_run_id, employment_id)
         VALUES ($1::uuid, 'emp-1')",
    )
    .bind(&run_id)
    .execute(&mut *first_transaction)
    .await
    .expect("first membership is allowed");

    let (started_sender, started_receiver) = oneshot::channel();
    let second_pool = pool.clone();
    let second_run_id = run_id.clone();
    let second_membership = tokio::spawn(async move {
        let mut connection = second_pool
            .acquire()
            .await
            .expect("acquire second connection");
        let mut transaction = connection.begin().await.expect("begin second transaction");
        started_sender.send(()).expect("notify first transaction");
        let result = sqlx::query(
            "INSERT INTO payroll_run_employment (payroll_run_id, employment_id)
             VALUES ($1::uuid, 'emp-2')",
        )
        .bind(&second_run_id)
        .execute(&mut *transaction)
        .await;
        transaction
            .rollback()
            .await
            .expect("rollback second transaction");
        result
    });

    started_receiver.await.expect("second transaction started");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !second_membership.is_finished(),
        "the second insert must wait for the first transaction's run-row lock"
    );

    first_transaction
        .commit()
        .await
        .expect("commit first transaction");
    let result = second_membership.await.expect("join second task");
    assert!(
        result.is_err(),
        "the second concurrent membership must be refused after the first commits"
    );
}

#[sqlx::test]
async fn reclassifying_a_multi_member_ordinary_run_as_a_correction_is_refused(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    an_employer_and_two_employments(&mut conn).await;

    let run_id: String = sqlx::query_scalar(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status, created_by)
         VALUES
            ('employer-1', '2026-03-01', '2026-03-31', '2026-04-05', 'ordinary', 'draft', 'actor')
         RETURNING id::text",
    )
    .fetch_one(&mut *conn)
    .await
    .expect("insert ordinary run");

    sqlx::query(
        "INSERT INTO payroll_run_employment (payroll_run_id, employment_id)
         VALUES ($1::uuid, 'emp-1'), ($1::uuid, 'emp-2')",
    )
    .bind(&run_id)
    .execute(&mut *conn)
    .await
    .expect("insert ordinary memberships");

    let result = sqlx::query(
        "UPDATE payroll_run
         SET kind = 'correction', correction_reason = 'reclassified'
         WHERE id = $1::uuid",
    )
    .bind(&run_id)
    .execute(&mut *conn)
    .await;

    assert!(
        result.is_err(),
        "a multi-member ordinary run cannot be reclassified as a Correction run"
    );
}

#[sqlx::test]
async fn liveness_must_match_the_finalized_payrolls_employment_and_period(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    an_employer_and_two_employments(&mut conn).await;

    let run_id: String = sqlx::query_scalar(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status, created_by)
         VALUES
            ('employer-1', '2026-03-01', '2026-03-31', '2026-04-05', 'ordinary', 'finalized', 'actor')
         RETURNING id::text",
    )
    .fetch_one(&mut *conn)
    .await
    .expect("insert finalized ordinary run");

    sqlx::query(
        "INSERT INTO payroll_run_employment (payroll_run_id, employment_id)
         VALUES ($1::uuid, 'emp-1')",
    )
    .bind(&run_id)
    .execute(&mut *conn)
    .await
    .expect("emp-1 is a member of the finalized run");

    let finalized_payroll_id: String = sqlx::query_scalar(
        "INSERT INTO finalized_payroll
            (payroll_run_id, employment_id, employer_id, period_start, period_end, tax_year,
             payroll_input_json, payroll_rules_json, payroll_calculation_json,
             taxable_remuneration, paye, paye_table_id, ssc_rules_id, salt_version, finalized_by)
         VALUES
            ($1::uuid, 'emp-1', 'employer-1', '2026-03-01', '2026-03-31', 2026,
             '{}', '{}', '{}', 15000, 1200, 'paye-1', 'ssc-1', '0.1.0+gdeadbeef', 'actor')
         RETURNING id::text",
    )
    .bind(&run_id)
    .fetch_one(&mut *conn)
    .await
    .expect("insert finalized payroll");

    let result = sqlx::query(
        "INSERT INTO live_finalized_payroll (employment_id, period_end, finalized_payroll_id)
         VALUES ('emp-2', '2026-03-31', $1::uuid)",
    )
    .bind(&finalized_payroll_id)
    .execute(&mut *conn)
    .await;

    assert!(
        result.is_err(),
        "liveness cannot identify a different Employment than its finalized payroll"
    );
}

/// The uniqueness rules §11 lists for the four fact tables, the working
/// calculation, the reversal and the liveness table. Each one is a single
/// `UNIQUE` or `PRIMARY KEY` whose absence would show up not as a failing
/// test but as a second, contradictory row nobody notices for a year.
#[sqlx::test]
async fn each_fact_is_recorded_at_most_once(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    an_employer_and_two_employments(&mut conn).await;

    let run_id: String = sqlx::query_scalar(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status, created_by)
         VALUES
            ('employer-1', '2026-03-01', '2026-03-31', '2026-04-05', 'ordinary', 'finalized', 'actor')
         RETURNING id::text",
    )
    .fetch_one(&mut *conn)
    .await
    .expect("insert ordinary run");

    sqlx::query(
        "INSERT INTO payroll_run_employment (payroll_run_id, employment_id)
         VALUES ($1::uuid, 'emp-1')",
    )
    .bind(&run_id)
    .execute(&mut *conn)
    .await
    .expect("emp-1 is a member of the run");

    let finalized_payroll_id: String = sqlx::query_scalar(
        "INSERT INTO finalized_payroll
            (payroll_run_id, employment_id, employer_id, period_start, period_end, tax_year,
             payroll_input_json, payroll_rules_json, payroll_calculation_json,
             taxable_remuneration, paye, paye_table_id, ssc_rules_id, salt_version, finalized_by)
         VALUES
            ($1::uuid, 'emp-1', 'employer-1', '2026-03-01', '2026-03-31', 2026,
             '{}', '{}', '{}', 15000, 1200, 'paye-1', 'ssc-1', '0.1.0+gdeadbeef', 'actor')
         RETURNING id::text",
    )
    .bind(&run_id)
    .fetch_one(&mut *conn)
    .await
    .expect("insert finalized payroll");

    let facts: [(&str, &str, &str); 7] = [
        (
            "one OpeningBalance per (Employment, TaxYear)",
            "INSERT INTO opening_balance
                (employment_id, tax_year, first_salt_period_end,
                 prior_taxable_remuneration, prior_paye, created_by)
             VALUES ('emp-1', 2026, '2026-10-31', 0, 0, 'actor')",
            "",
        ),
        (
            "one PriorEmploymentDeclaration per (Employment, TaxYear)",
            "INSERT INTO prior_employment_declaration
                (employment_id, tax_year, status, declared_by)
             VALUES ('emp-1', 2026, 'confirmed_none', 'actor')",
            "",
        ),
        (
            "one UnsupportedDeductionDeclaration per (Employment, effective_from)",
            "INSERT INTO unsupported_deduction_declaration
                (employment_id, effective_from, status, declared_by)
             VALUES ('emp-1', '2026-03-01', 'confirmed_none', 'actor')",
            "",
        ),
        (
            "one CompensationTerms row per (Employment, effective_from)",
            "INSERT INTO compensation_terms
                (employment_id, effective_from, basic_pay, created_by)
             VALUES ('emp-1', '2026-03-01', 1500000, 'actor')",
            "",
        ),
        (
            "one WorkingCalculation per (run, Employment)",
            "INSERT INTO working_payroll_calculation
                (payroll_run_id, employment_id, payroll_input_json, payroll_rules_json,
                 payroll_calculation_json, calculated_by)
             VALUES ($1::uuid, 'emp-1', '{}', '{}', '{}', 'actor')",
            "run",
        ),
        (
            "one Reversal per FinalizedPayroll",
            "INSERT INTO reversal (finalized_payroll_id, reversed_by, reason)
             VALUES ($1::uuid, 'actor', 'wrong salary')",
            "finalized",
        ),
        (
            "one Live FinalizedPayroll per (Employment, period end)",
            "INSERT INTO live_finalized_payroll
                (employment_id, period_end, finalized_payroll_id)
             VALUES ('emp-1', '2026-03-31', $1::uuid)",
            "finalized",
        ),
    ];

    for (fact, statement, binding) in facts {
        for attempt in ["first", "second"] {
            let query = match binding {
                "run" => sqlx::query(statement).bind(&run_id),
                "finalized" => sqlx::query(statement).bind(&finalized_payroll_id),
                _ => sqlx::query(statement),
            };
            let result = query.execute(&mut *conn).await;

            if attempt == "first" {
                result.unwrap_or_else(|err| panic!("stating {fact} once must work: {err:?}"));
            } else {
                let err = result
                    .err()
                    .unwrap_or_else(|| panic!("stating {fact} twice must be refused"));
                assert!(
                    is_unique_violation(&err),
                    "expected a unique_violation while restating {fact}, got {err:?}"
                );
            }
        }
    }
}

/// §11: `CHECK correction_reason non-empty when kind = Correction`. A
/// Correction run exists only because someone decided history was wrong, so a
/// run that cannot say why is the one shape this column exists to forbid.
#[sqlx::test]
async fn a_correction_run_must_say_why_it_exists(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    an_employer_and_two_employments(&mut conn).await;

    for (case, correction_reason) in [("a missing", "NULL"), ("an empty", "''")] {
        let result = sqlx::query(&format!(
            "INSERT INTO payroll_run
                (employer_id, period_start, period_end, pay_date, kind, status,
                 correction_reason, created_by)
             VALUES
                ('employer-1', '2026-03-01', '2026-03-31', '2026-06-05', 'correction', 'draft',
                 {correction_reason}, 'actor')"
        ))
        .execute(&mut *conn)
        .await;

        assert!(
            result.is_err(),
            "a Correction run with {case} reason must be refused"
        );
    }

    let ordinary_with_a_reason = sqlx::query(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status,
             correction_reason, created_by)
         VALUES
            ('employer-1', '2026-03-01', '2026-03-31', '2026-04-05', 'ordinary', 'draft',
             'ordinary runs have nothing to correct', 'actor')",
    )
    .execute(&mut *conn)
    .await;

    assert!(
        ordinary_with_a_reason.is_err(),
        "an Ordinary run must carry no correction reason"
    );
}

/// §4.8: a removal is the deliberate, reasoned act that keeps a silent
/// omission from being possible, so the three removal columns are set
/// together or not at all, and the reason is never blank.
#[sqlx::test]
async fn a_removal_from_a_run_is_attributed_and_reasoned(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    an_employer_and_two_employments(&mut conn).await;

    let run_id: String = sqlx::query_scalar(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status, created_by)
         VALUES
            ('employer-1', '2026-03-01', '2026-03-31', '2026-04-05', 'ordinary', 'draft', 'actor')
         RETURNING id::text",
    )
    .fetch_one(&mut *conn)
    .await
    .expect("insert ordinary run");

    sqlx::query(
        "INSERT INTO payroll_run_employment (payroll_run_id, employment_id)
         VALUES ($1::uuid, 'emp-1')",
    )
    .bind(&run_id)
    .execute(&mut *conn)
    .await
    .expect("insert membership");

    for (case, assignment) in [
        (
            "a removal with no actor",
            "removed_at = now(), removal_reason = 'unpaid leave'",
        ),
        (
            "a removal with no reason",
            "removed_at = now(), removed_by = 'actor'",
        ),
        (
            "a removal with an empty reason",
            "removed_at = now(), removed_by = 'actor', removal_reason = ''",
        ),
    ] {
        let result = sqlx::query(&format!(
            "UPDATE payroll_run_employment SET {assignment} WHERE payroll_run_id = $1::uuid"
        ))
        .bind(&run_id)
        .execute(&mut *conn)
        .await;

        assert!(result.is_err(), "{case} must be refused");
    }

    sqlx::query(
        "UPDATE payroll_run_employment
         SET removed_at = now(), removed_by = 'actor', removal_reason = 'unpaid leave'
         WHERE payroll_run_id = $1::uuid",
    )
    .bind(&run_id)
    .execute(&mut *conn)
    .await
    .expect("a reasoned, attributed removal is the supported act");
}

/// A run-scoped row for an Employment the run does not include would have
/// bypassed the one place inclusion is decided (§4.8) — auto-proposal and
/// reasoned removal for an Ordinary run, one explicit member for a Correction
/// run. The database, not a call site, is what makes that unrepresentable.
#[sqlx::test]
async fn run_scoped_rows_require_run_membership(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    an_employer_and_two_employments(&mut conn).await;

    let run_id: String = sqlx::query_scalar(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status, created_by)
         VALUES
            ('employer-1', '2026-03-01', '2026-03-31', '2026-04-05', 'ordinary', 'draft', 'actor')
         RETURNING id::text",
    )
    .fetch_one(&mut *conn)
    .await
    .expect("insert ordinary run");

    for (row, statement) in [
        (
            "an Earning line",
            "INSERT INTO payroll_run_earning (payroll_run_id, employment_id, line, earning_json)
             VALUES ($1::uuid, 'emp-1', 1, '{}')",
        ),
        (
            "a WorkingCalculation",
            "INSERT INTO working_payroll_calculation
                (payroll_run_id, employment_id, payroll_input_json, payroll_rules_json,
                 payroll_calculation_json, calculated_by)
             VALUES ($1::uuid, 'emp-1', '{}', '{}', '{}', 'actor')",
        ),
        (
            "a FinalizedPayroll",
            "INSERT INTO finalized_payroll
                (payroll_run_id, employment_id, employer_id, period_start, period_end, tax_year,
                 payroll_input_json, payroll_rules_json, payroll_calculation_json,
                 taxable_remuneration, paye, paye_table_id, ssc_rules_id, salt_version,
                 finalized_by)
             VALUES
                ($1::uuid, 'emp-1', 'employer-1', '2026-03-01', '2026-03-31', 2026,
                 '{}', '{}', '{}', 15000, 1200, 'paye-1', 'ssc-1', '0.1.0+gdeadbeef', 'actor')",
        ),
    ] {
        let result = sqlx::query(statement)
            .bind(&run_id)
            .execute(&mut *conn)
            .await;

        assert!(
            result.is_err(),
            "{row} for an Employment the run does not include must be refused"
        );
    }
}

/// A FinalizedPayroll names its Employment, its Employer and its PayPeriod
/// independently of the run it came from. No role may correct this table, so a
/// disagreement between those columns and the run would be permanent.
#[sqlx::test]
async fn a_finalized_payroll_agrees_with_the_run_it_came_from(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    an_employer_and_two_employments(&mut conn).await;

    sqlx::query(
        "INSERT INTO employer (id, period_end_day_kind, period_end_day_value, created_by)
         VALUES ('employer-2', 'day', 25, 'actor')",
    )
    .execute(&mut *conn)
    .await
    .expect("insert a second employer");

    let run_id: String = sqlx::query_scalar(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status, created_by)
         VALUES
            ('employer-1', '2026-03-01', '2026-03-31', '2026-04-05', 'ordinary', 'finalized', 'actor')
         RETURNING id::text",
    )
    .fetch_one(&mut *conn)
    .await
    .expect("insert ordinary run");

    sqlx::query(
        "INSERT INTO payroll_run_employment (payroll_run_id, employment_id)
         VALUES ($1::uuid, 'emp-1')",
    )
    .bind(&run_id)
    .execute(&mut *conn)
    .await
    .expect("emp-1 is a member of the run");

    let insert = |employer: &str, period_start: &str, period_end: &str| {
        format!(
            "INSERT INTO finalized_payroll
                (payroll_run_id, employment_id, employer_id, period_start, period_end, tax_year,
                 payroll_input_json, payroll_rules_json, payroll_calculation_json,
                 taxable_remuneration, paye, paye_table_id, ssc_rules_id, salt_version,
                 finalized_by)
             VALUES
                ($1::uuid, 'emp-1', '{employer}', '{period_start}', '{period_end}', 2026,
                 '{{}}', '{{}}', '{{}}', 15000, 1200, 'paye-1', 'ssc-1', '0.1.0+gdeadbeef',
                 'actor')"
        )
    };

    for (case, statement) in [
        (
            "an Employer that is not the Employment's",
            insert("employer-2", "2026-03-01", "2026-03-31"),
        ),
        (
            "a PayPeriod that is not the run's",
            insert("employer-1", "2026-04-01", "2026-04-30"),
        ),
        (
            "a period that ends before it starts",
            insert("employer-1", "2026-03-31", "2026-03-01"),
        ),
    ] {
        let result = sqlx::query(&statement)
            .bind(&run_id)
            .execute(&mut *conn)
            .await;

        assert!(
            result.is_err(),
            "a FinalizedPayroll naming {case} must be refused"
        );
    }

    sqlx::query(&insert("employer-1", "2026-03-01", "2026-03-31"))
        .bind(&run_id)
        .execute(&mut *conn)
        .await
        .expect("a FinalizedPayroll agreeing with its run is the supported shape");
}

/// §4.4 is explicit that CompensationTerms carries `effective_from` only: a row
/// is in force until the next row's `effective_from`. A second column stating
/// the same boundary could disagree with the first, making a gap or an overlap
/// between two rows representable.
#[sqlx::test]
async fn compensation_terms_state_only_when_they_begin(pool: PgPool) {
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns
         WHERE table_schema = 'public' AND table_name = 'compensation_terms'",
    )
    .fetch_all(&pool)
    .await
    .expect("read compensation_terms columns");

    assert!(
        columns.iter().any(|c| c == "effective_from"),
        "CompensationTerms must state when a row begins, found {columns:?}"
    );
    assert!(
        !columns.iter().any(|c| c == "effective_until"),
        "a row is in force until the next row's effective_from, so no column may \
         state an end independently, found {columns:?}"
    );
}
