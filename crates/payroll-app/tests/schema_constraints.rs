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

const NOT_NULL_VIOLATION: &str = "23502";

fn is_not_null_violation(err: &sqlx::Error) -> bool {
    matches!(err, sqlx::Error::Database(db_err) if db_err.code().as_deref() == Some(NOT_NULL_VIOLATION))
}

async fn an_employer_and_two_employments(conn: &mut sqlx::PgConnection) {
    sqlx::query(
        "INSERT INTO employer (id, name, period_end_day_kind, period_end_day_value, created_by)
         VALUES ('employer-1', 'Employer', 'day', 25, 'actor')",
    )
    .execute(&mut *conn)
    .await
    .expect("insert employer");

    sqlx::query(
        "INSERT INTO person (id, employer_id, full_name, created_by)
         VALUES ('person-1', 'employer-1', 'Test Person 1', 'actor'),
                ('person-2', 'employer-1', 'Test Person 2', 'actor')",
    )
    .execute(&mut *conn)
    .await
    .expect("insert persons");

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

/// The Ordinary March run, its membership for `emp-1`, and the
/// `FinalizedPayroll` it produced — the row every replacement below names as
/// its target. Returns `(run id, finalized payroll id)`.
async fn a_finalized_march_for_emp_1(conn: &mut sqlx::PgConnection) -> (String, String) {
    let run_id: String = sqlx::query_scalar(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status, created_by)
         VALUES
            ('employer-1', '2026-03-01', '2026-03-31', '2026-04-05', 'ordinary', 'finalized',
             'actor')
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
    .expect("both Employments are members of the Ordinary run");

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
    .expect("insert the original finalized payroll");

    (run_id, finalized_payroll_id)
}

/// A Correction run for `period`, holding `employment_id` alone, already
/// marked finalized so a `FinalizedPayroll` may be written against it.
async fn a_correction_run_holding(
    conn: &mut sqlx::PgConnection,
    employment_id: &str,
    period_start: &str,
    period_end: &str,
    replaces: Option<&str>,
) -> Result<String, sqlx::Error> {
    let run_id: String = sqlx::query_scalar(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status, correction_reason,
             created_by)
         VALUES
            ('employer-1', $1::date, $2::date, '2026-07-05', 'correction', 'finalized',
             'fix march', 'actor')
         RETURNING id::text",
    )
    .bind(period_start)
    .bind(period_end)
    .fetch_one(&mut *conn)
    .await?;

    sqlx::query(
        "INSERT INTO payroll_run_employment
            (payroll_run_id, employment_id, replaces_finalized_payroll_id)
         VALUES ($1::uuid, $2, $3::uuid)",
    )
    .bind(&run_id)
    .bind(employment_id)
    .bind(replaces)
    .execute(&mut *conn)
    .await?;

    Ok(run_id)
}

/// §4.8: a replacement and the record it replaces are one Employment's
/// payroll. Rust checks this before a Correction run is even calculated, but
/// `finalized_payroll` is the table no role may correct — a replacement
/// pointing at someone else's record would be a permanent lie about what was
/// replaced.
#[sqlx::test]
async fn a_replacement_names_a_target_for_its_own_employment(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    an_employer_and_two_employments(&mut conn).await;
    let (_, march_for_emp_1) = a_finalized_march_for_emp_1(&mut conn).await;

    let other_employments_run =
        a_correction_run_holding(&mut conn, "emp-2", "2026-03-01", "2026-03-31", None)
            .await
            .expect("a Correction run for emp-2's own March");

    let result = sqlx::query(
        "INSERT INTO finalized_payroll
            (payroll_run_id, employment_id, employer_id, period_start, period_end, tax_year,
             replaces_finalized_payroll_id,
             payroll_input_json, payroll_rules_json, payroll_calculation_json,
             taxable_remuneration, paye, paye_table_id, ssc_rules_id, salt_version, finalized_by)
         VALUES
            ($1::uuid, 'emp-2', 'employer-1', '2026-03-01', '2026-03-31', 2026, $2::uuid,
             '{}', '{}', '{}', 16000, 1300, 'paye-1', 'ssc-1', '0.1.0+gdeadbeef', 'actor')",
    )
    .bind(&other_employments_run)
    .bind(&march_for_emp_1)
    .execute(&mut *conn)
    .await;

    assert!(
        result.is_err(),
        "a replacement cannot name another Employment's FinalizedPayroll"
    );
}

/// §4.8, the other half of the same pair: one Employment, but the record it
/// replaces must be that Employment's payroll for *this* PayPeriod. A
/// correction is per period, and ADR-0002's chain is per period with it.
#[sqlx::test]
async fn a_replacement_names_a_target_for_its_own_period(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    an_employer_and_two_employments(&mut conn).await;
    let (_, march_for_emp_1) = a_finalized_march_for_emp_1(&mut conn).await;

    let april_run = a_correction_run_holding(&mut conn, "emp-1", "2026-04-01", "2026-04-30", None)
        .await
        .expect("a Correction run for emp-1's April");

    let result = sqlx::query(
        "INSERT INTO finalized_payroll
            (payroll_run_id, employment_id, employer_id, period_start, period_end, tax_year,
             replaces_finalized_payroll_id,
             payroll_input_json, payroll_rules_json, payroll_calculation_json,
             taxable_remuneration, paye, paye_table_id, ssc_rules_id, salt_version, finalized_by)
         VALUES
            ($1::uuid, 'emp-1', 'employer-1', '2026-04-01', '2026-04-30', 2026, $2::uuid,
             '{}', '{}', '{}', 16000, 1300, 'paye-1', 'ssc-1', '0.1.0+gdeadbeef', 'actor')",
    )
    .bind(&april_run)
    .bind(&march_for_emp_1)
    .execute(&mut *conn)
    .await;

    assert!(
        result.is_err(),
        "a replacement cannot name a FinalizedPayroll for another PayPeriod"
    );
}

/// §4.8: on membership, `replaces_finalized_payroll_id` is the declared
/// target and it is Correction-only. The period it must match lives on
/// `payroll_run`, so the database holds the half it can — the Employment.
#[sqlx::test]
async fn a_declared_target_belongs_to_the_declaring_employment(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    an_employer_and_two_employments(&mut conn).await;
    let (_, march_for_emp_1) = a_finalized_march_for_emp_1(&mut conn).await;

    let result = a_correction_run_holding(
        &mut conn,
        "emp-2",
        "2026-03-01",
        "2026-03-31",
        Some(&march_for_emp_1),
    )
    .await;

    assert!(
        result.is_err(),
        "a Correction run cannot declare another Employment's FinalizedPayroll as its target"
    );
}

/// §4.8: only a Correction run declares a target. Finalization copies each
/// member's declared target into its `FinalizedPayroll` by kind, and an
/// Ordinary member's is always NULL — so a membership row that carried one
/// anyway would put recorded lineage on a row that replaced nothing.
#[sqlx::test]
async fn only_a_correction_run_declares_a_replacement_target(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    an_employer_and_two_employments(&mut conn).await;
    let (ordinary_run_id, march_for_emp_1) = a_finalized_march_for_emp_1(&mut conn).await;

    let declared_on_an_ordinary_run = sqlx::query(
        "UPDATE payroll_run_employment SET replaces_finalized_payroll_id = $2::uuid
         WHERE payroll_run_id = $1::uuid AND employment_id = 'emp-1'",
    )
    .bind(&ordinary_run_id)
    .bind(&march_for_emp_1)
    .execute(&mut *conn)
    .await;
    assert!(
        declared_on_an_ordinary_run.is_err(),
        "an Ordinary run's membership row cannot declare a replacement target"
    );

    // The other direction: a Correction run holding a declared target,
    // reclassified as Ordinary.
    let correction_run_id = a_correction_run_holding(
        &mut conn,
        "emp-1",
        "2026-03-01",
        "2026-03-31",
        Some(&march_for_emp_1),
    )
    .await
    .expect("a Correction run may declare emp-1's own March record");

    let reclassified = sqlx::query("UPDATE payroll_run SET kind = 'ordinary' WHERE id = $1::uuid")
        .bind(&correction_run_id)
        .execute(&mut *conn)
        .await;
    assert!(
        reclassified.is_err(),
        "a run holding a declared target cannot become Ordinary"
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

    for (case, correction_reason) in [
        ("a missing", "NULL"),
        ("an empty", "''"),
        ("a whitespace-only", "'   '"),
    ] {
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

/// §4.5 guard 2, and ADR-0005's rule that a `PayPeriod` belongs to the
/// `TaxYear` its **end** date falls in: January and February belong to the
/// year that started the previous March. §7 reads `first_salt_period_end` as
/// the boundary of the row's own `TaxYear` and §8 adds the row's figures to
/// that one year, so a row whose boundary belongs to another year would
/// answer both questions wrongly and silently.
#[sqlx::test]
async fn an_opening_balance_boundary_belongs_to_its_own_tax_year(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    an_employer_and_two_employments(&mut conn).await;

    // 31 January 2027 is inside the TaxYear starting March 2026, not the one
    // starting March 2027 -- the reading a bare calendar year gets wrong.
    let january_belongs_to_the_previous_march = sqlx::query(
        "INSERT INTO opening_balance
            (employment_id, tax_year, first_salt_period_end,
             prior_taxable_remuneration, prior_paye, created_by)
         VALUES ('emp-1', 2026, '2027-01-31', 0, 0, 'actor')",
    )
    .execute(&mut *conn)
    .await;

    assert!(
        january_belongs_to_the_previous_march.is_ok(),
        "a January boundary belongs to the TaxYear that started the previous March, \
         got {january_belongs_to_the_previous_march:?}"
    );

    for (case, tax_year, boundary) in [
        (
            "a boundary a whole year after its TaxYear",
            2026,
            "2027-10-31",
        ),
        (
            "a boundary a whole year before its TaxYear",
            2026,
            "2025-10-31",
        ),
        (
            "a January boundary read as its own calendar year",
            2027,
            "2027-01-31",
        ),
    ] {
        let result = sqlx::query(&format!(
            "INSERT INTO opening_balance
                (employment_id, tax_year, first_salt_period_end,
                 prior_taxable_remuneration, prior_paye, created_by)
             VALUES ('emp-2', {tax_year}, '{boundary}', 0, 0, 'actor')"
        ))
        .execute(&mut *conn)
        .await;

        assert!(result.is_err(), "{case} must be refused");
    }
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
        (
            "a removal whose reason is only whitespace",
            "removed_at = now(), removed_by = 'actor', removal_reason = '   '",
        ),
        (
            "a removal whose actor is only whitespace",
            "removed_at = now(), removed_by = ' ', removal_reason = 'unpaid leave'",
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
        "INSERT INTO employer (id, name, period_end_day_kind, period_end_day_value, created_by)
         VALUES ('employer-2', 'Employer', 'day', 25, 'actor')",
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

/// §8 sums `finalized_payroll.taxable_remuneration` and `.paye` for every
/// later period of the TaxYear, so these two columns are the only place a
/// stored Money is read back as arithmetic rather than as a display figure.
/// A negative or fractional cent here would be rounded away by the reader's
/// cast and change a PAYE figure for the rest of the year, in a table no
/// role may correct. The CHECK is what makes the reader's `Money::from_cents`
/// a schema guarantee rather than a hope about every writer (migration 0024).
#[sqlx::test]
async fn a_finalized_payroll_holds_whole_non_negative_cents(pool: PgPool) {
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

    let insert = |taxable_remuneration: &str, paye: &str| {
        format!(
            "INSERT INTO finalized_payroll
                (payroll_run_id, employment_id, employer_id, period_start, period_end, tax_year,
                 payroll_input_json, payroll_rules_json, payroll_calculation_json,
                 taxable_remuneration, paye, paye_table_id, ssc_rules_id, salt_version,
                 finalized_by)
             VALUES
                ($1::uuid, 'emp-1', 'employer-1', '2026-03-01', '2026-03-31', 2026,
                 '{{}}', '{{}}', '{{}}', {taxable_remuneration}, {paye}, 'paye-1', 'ssc-1',
                 '0.1.0+gdeadbeef', 'actor')"
        )
    };

    // A fraction of a cent is not refused so much as unrepresentable: the
    // columns are BIGINT, so there is no value of them a `Money` cannot
    // decode. That half of the guarantee is the type; the CHECK below is
    // the other half.
    let types: Vec<String> = sqlx::query_scalar(
        "SELECT data_type FROM information_schema.columns
         WHERE table_name = 'finalized_payroll'
           AND column_name IN ('taxable_remuneration', 'paye')
         ORDER BY column_name",
    )
    .fetch_all(&mut *conn)
    .await
    .expect("read the column types");
    assert_eq!(types, vec!["bigint".to_string(), "bigint".to_string()]);

    for (case, statement) in [
        ("a negative taxable remuneration", insert("-1", "1200")),
        ("a negative PAYE", insert("15000", "-1")),
    ] {
        let result = sqlx::query(&statement)
            .bind(&run_id)
            .execute(&mut *conn)
            .await;

        assert!(
            result.is_err(),
            "a FinalizedPayroll holding {case} must be refused"
        );
    }

    sqlx::query(&insert("15000", "1200"))
        .bind(&run_id)
        .execute(&mut *conn)
        .await
        .expect("whole, non-negative cents are the supported shape");
}

/// §8's year-to-date read cuts history by `period_end` and filters it by
/// `tax_year`, so the two columns answer one question together. A permanent
/// history row whose `tax_year` disagreed with its own `period_end` would be
/// summed into a year it does not belong to, or dropped from the one it does,
/// and `finalized_payroll` has no UPDATE grant to repair it with. ADR-0005
/// keys a PayPeriod's TaxYear on its end date alone, so January and February
/// belong to the year that started the previous March.
#[sqlx::test]
async fn a_finalized_payroll_belongs_to_its_own_tax_year(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    an_employer_and_two_employments(&mut conn).await;

    // A period ending 31 January 2027 belongs to the TaxYear starting March
    // 2026 -- the reading a bare calendar year gets wrong.
    let a_run = |period_start: &str, period_end: &str| {
        format!(
            "INSERT INTO payroll_run
                (employer_id, period_start, period_end, pay_date, kind, status, created_by)
             VALUES
                ('employer-1', '{period_start}', '{period_end}', '{period_end}', 'ordinary',
                 'finalized', 'actor')
             RETURNING id::text"
        )
    };
    let a_finalized_payroll =
        |run_id: &str, period_start: &str, period_end: &str, tax_year: i32| {
            format!(
                "INSERT INTO finalized_payroll
                (payroll_run_id, employment_id, employer_id, period_start, period_end, tax_year,
                 payroll_input_json, payroll_rules_json, payroll_calculation_json,
                 taxable_remuneration, paye, paye_table_id, ssc_rules_id, salt_version,
                 finalized_by)
             VALUES
                ('{run_id}'::uuid, 'emp-1', 'employer-1', '{period_start}', '{period_end}',
                 {tax_year}, '{{}}', '{{}}', '{{}}', 15000, 1200, 'paye-1', 'ssc-1',
                 '0.1.0+gdeadbeef', 'actor')"
            )
        };

    for (case, period_start, period_end, tax_year, is_accepted) in [
        (
            "a January period read as the TaxYear that started the previous March",
            "2027-01-01",
            "2027-01-31",
            2026,
            true,
        ),
        (
            "a January period read as its own calendar year",
            "2026-12-26",
            "2027-01-25",
            2027,
            false,
        ),
        (
            "an October period a whole year after its TaxYear",
            "2026-10-01",
            "2026-10-31",
            2025,
            false,
        ),
    ] {
        let run_id: String = sqlx::query_scalar(&a_run(period_start, period_end))
            .fetch_one(&mut *conn)
            .await
            .expect("insert the run the history came from");
        sqlx::query(
            "INSERT INTO payroll_run_employment (payroll_run_id, employment_id)
             VALUES ($1::uuid, 'emp-1')",
        )
        .bind(&run_id)
        .execute(&mut *conn)
        .await
        .expect("emp-1 is a member of the run");

        let result = sqlx::query(&a_finalized_payroll(
            &run_id,
            period_start,
            period_end,
            tax_year,
        ))
        .execute(&mut *conn)
        .await;

        assert_eq!(result.is_ok(), is_accepted, "{case} -- got {result:?}");
    }
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

/// §0.2 gives an Employer one human-readable name, so the application has
/// something to show a person other than an id (issue #40). `NOT NULL` is what
/// makes "has a name" a fact about every row rather than a habit of the one
/// use case that writes them: an Employer recorded without a name is an
/// Employer no screen can title.
#[sqlx::test]
async fn an_employer_is_always_named(pool: PgPool) {
    let result = sqlx::query(
        "INSERT INTO employer (id, period_end_day_kind, period_end_day_value, created_by)
         VALUES ('employer-1', 'day', 25, 'actor')",
    )
    .execute(&pool)
    .await;

    let error = result.expect_err("an Employer with no name must be refused");
    assert!(
        is_not_null_violation(&error),
        "expected a NOT NULL violation, got {error}"
    );
}

/// §10 makes the ActionLog "who did what and when", and §4.8 makes a removal
/// reason the thing an Employer needs six months later. Migration 0019 refused
/// the empty string for both; a value made only of whitespace answers those
/// questions with exactly as much, and neither is fixable afterwards — the
/// application role may not `UPDATE` the log at all (migration 0015), nor a
/// finalized payroll. Migration 0023 refuses it everywhere both kinds of
/// column appear.
#[sqlx::test]
async fn an_actor_and_a_reason_are_never_only_whitespace(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");
    an_employer_and_two_employments(&mut conn).await;

    for (case, statement) in [
        (
            "an Employer created by nobody",
            "INSERT INTO employer (id, name, period_end_day_kind, period_end_day_value, created_by)
             VALUES ('employer-2', 'Employer', 'day', 25, ' ')",
        ),
        (
            "an Employer named only whitespace",
            "INSERT INTO employer (id, name, period_end_day_kind, period_end_day_value, created_by)
             VALUES ('employer-3', ' ', 'day', 25, 'actor')",
        ),
        (
            "an Employer named only tabs and newlines",
            "INSERT INTO employer (id, name, period_end_day_kind, period_end_day_value, created_by)
             VALUES ('employer-4', E'\\t\\n', 'day', 25, 'actor')",
        ),
        (
            "an Employment created by nobody",
            "INSERT INTO employment (id, employer_id, person_id, start_date, created_by)
             VALUES ('emp-3', 'employer-1', 'person-3', '2026-03-01', '  ')",
        ),
        (
            "an Employment naming no Person",
            "INSERT INTO employment (id, employer_id, person_id, start_date, created_by)
             VALUES ('emp-4', 'employer-1', ' ', '2026-03-01', 'actor')",
        ),
        (
            "CompensationTerms recorded by nobody",
            "INSERT INTO compensation_terms
                (employment_id, effective_from, basic_pay, created_by)
             VALUES ('emp-1', '2026-02-26', 500000, ' ')",
        ),
        (
            "a PayrollRun created by nobody",
            "INSERT INTO payroll_run
                (employer_id, period_start, period_end, pay_date, kind, status, created_by)
             VALUES
                ('employer-1', '2026-03-01', '2026-03-31', '2026-04-05', 'ordinary', 'draft', ' ')",
        ),
        (
            "an ActionLog entry naming no actor",
            "INSERT INTO action_log_entry
                (employer_id, actor, action_type, target_type, target_id)
             VALUES ('employer-1', ' ', 'payroll_run_created', 'payroll_run', 'some-id')",
        ),
        (
            "an ActionLog entry pointing at nothing",
            "INSERT INTO action_log_entry
                (employer_id, actor, action_type, target_type, target_id)
             VALUES ('employer-1', 'actor', 'payroll_run_created', 'payroll_run', '  ')",
        ),
    ] {
        let result = sqlx::query(statement).execute(&mut *conn).await;

        assert!(result.is_err(), "{case} must be refused");
    }
}

/// Issue #41 makes an Operator's email unique case-insensitively, and issue
/// #38 §0.12a makes a sign-in refuse without saying which account exists. Both
/// rest on two emails a person reads as one resolving to one row. Folding case
/// alone does not do that — ` alice@x` and `alice@x` fold apart — so the
/// database refuses surrounding whitespace outright rather than trusting the
/// one use case that writes the column to have trimmed it. Stated here as a
/// schema fact because two concurrent inserts cannot be made to agree by
/// application discipline.
#[sqlx::test]
async fn an_operator_email_that_folds_to_another_is_refused(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");

    sqlx::query(
        "INSERT INTO operator (id, email, display_name, password_verifier)
         VALUES (gen_random_uuid(), 'Alice@Example.com', 'Alice', '$argon2id$verifier')",
    )
    .execute(&mut *conn)
    .await
    .expect("the first Operator is recorded with its own capitalisation");

    let collision = sqlx::query(
        "INSERT INTO operator (id, email, display_name, password_verifier)
         VALUES (gen_random_uuid(), 'alice@example.com', 'Alice Two', '$argon2id$verifier')",
    )
    .execute(&mut *conn)
    .await;

    let error = collision.expect_err("a differently-cased duplicate email must be refused");
    assert!(
        is_unique_violation(&error),
        "expected a unique violation, got {error}"
    );
}

/// The other half of that rule: a padded email is not a different email, so
/// the padding never reaches a row in the first place.
#[sqlx::test]
async fn an_operator_is_never_recorded_with_a_blank_or_padded_name_or_email(pool: PgPool) {
    let mut conn = pool.acquire().await.expect("acquire connection");

    for (case, statement) in [
        (
            "an Operator with a blank email",
            "INSERT INTO operator (id, email, display_name, password_verifier)
             VALUES (gen_random_uuid(), ' ', 'Alice', '$argon2id$verifier')",
        ),
        (
            "an Operator with an email of only tabs and newlines",
            "INSERT INTO operator (id, email, display_name, password_verifier)
             VALUES (gen_random_uuid(), E'\\t\\n', 'Alice', '$argon2id$verifier')",
        ),
        (
            "an Operator with a leading-space email",
            "INSERT INTO operator (id, email, display_name, password_verifier)
             VALUES (gen_random_uuid(), ' alice@example.com', 'Alice', '$argon2id$verifier')",
        ),
        (
            "an Operator with a trailing-tab email",
            "INSERT INTO operator (id, email, display_name, password_verifier)
             VALUES (gen_random_uuid(), E'alice@example.com\\t', 'Alice', '$argon2id$verifier')",
        ),
        (
            "an Operator with a blank display name",
            "INSERT INTO operator (id, email, display_name, password_verifier)
             VALUES (gen_random_uuid(), 'alice@example.com', ' ', '$argon2id$verifier')",
        ),
        (
            "an Operator with a padded display name",
            "INSERT INTO operator (id, email, display_name, password_verifier)
             VALUES (gen_random_uuid(), 'alice@example.com', 'Alice ', '$argon2id$verifier')",
        ),
        (
            "an Operator with a blank password verifier",
            "INSERT INTO operator (id, email, display_name, password_verifier)
             VALUES (gen_random_uuid(), 'alice@example.com', 'Alice', ' ')",
        ),
        (
            "an Operator with a status the design does not name",
            "INSERT INTO operator (id, email, display_name, password_verifier, status)
             VALUES (gen_random_uuid(), 'alice@example.com', 'Alice', '$argon2id$v', 'deleted')",
        ),
        (
            "an Operator with a negative failed-attempt count",
            "INSERT INTO operator
                (id, email, display_name, password_verifier, failed_attempt_count)
             VALUES (gen_random_uuid(), 'alice@example.com', 'Alice', '$argon2id$v', -1)",
        ),
    ] {
        let result = sqlx::query(statement).execute(&mut *conn).await;

        assert!(result.is_err(), "{case} must be refused");
    }
}
