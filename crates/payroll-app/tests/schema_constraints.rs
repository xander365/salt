//! Proves the constraints from docs/domain/payroll-run-persistence.md §11
//! that are least obvious from reading the migrations alone: a trigger
//! (not a plain `CHECK`, since the rule spans two tables) and two `UNIQUE`
//! constraints whose absence would be a silent, hard-to-notice regression.

use sqlx::PgPool;

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
