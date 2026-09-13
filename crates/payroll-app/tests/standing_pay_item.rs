//! Proves the use cases issue #79 introduces: `create_standing_pay_item`,
//! `end_standing_pay_item`, and `create_ordinary_payroll_run`'s own new
//! behaviour of proposing every `StandingPayItem` in force as a `standing`
//! pay line (`docs/domain/payroll-run-persistence.md` §0's own words for
//! `StandingPayItem`).

use std::time::Duration;

use chrono::NaiveDate;
use payroll::{DayOfMonth, EarningInstruction, Money, PayPeriod, PayrollError, PeriodEndDay};
use payroll_app::{
    EmploymentPerson, PayrollAppError, SaltDatabase, StandingPayItemInstruction,
    add_employment_to_correction_run, create_correction_run, create_employer, create_employment,
    create_ordinary_payroll_run, create_standing_pay_item, end_standing_pay_item,
    get_payroll_run_detail, set_run_pay_lines,
};
use sqlx::{PgPool, Row};

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// A 26th-to-25th monthly schedule, matching `ordinary_payroll_run.rs`.
fn twenty_sixth_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap()))
}

/// 2026-01-26 to 2026-02-25, one of `twenty_sixth_schedule()`'s own periods.
fn march_period() -> PayPeriod {
    PayPeriod::new(date(2026, 1, 26), date(2026, 2, 25)).unwrap()
}

/// The period immediately after `march_period()`.
fn april_period() -> PayPeriod {
    PayPeriod::new(date(2026, 2, 26), date(2026, 3, 25)).unwrap()
}

fn allowance(cents: i64) -> EarningInstruction {
    EarningInstruction::TaxableAllowance {
        amount: Money::from_cents(cents).unwrap(),
        label: None,
    }
}

fn standing_allowance(cents: i64) -> StandingPayItemInstruction {
    StandingPayItemInstruction::TaxableAllowance {
        amount: Money::from_cents(cents).unwrap(),
        label: None,
    }
}

fn standing_medical_aid(cents: i64) -> StandingPayItemInstruction {
    StandingPayItemInstruction::MedicalAidPremium(Money::from_cents(cents).unwrap())
}

async fn an_employer(db: &SaltDatabase) -> payroll::EmployerId {
    create_employer(db, "Employer", twenty_sixth_schedule(), "actor")
        .await
        .unwrap()
}

async fn an_employment(
    db: &SaltDatabase,
    employer_id: &payroll::EmployerId,
) -> payroll::EmploymentId {
    create_employment(
        db,
        employer_id,
        EmploymentPerson::New("Person".to_string()),
        date(2025, 1, 1),
        None,
        "actor",
    )
    .await
    .unwrap()
    .1
}

// ---- CreateStandingPayItem ----

#[sqlx::test]
async fn create_standing_pay_item_records_the_item(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;

    let item_id = create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(50_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();

    let row = sqlx::query(
        "SELECT employment_id, effective_from, ended_at, ended_by, ended_reason
         FROM standing_pay_item WHERE id = $1::uuid",
    )
    .bind(item_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>(0), employment_id.as_str());
    assert_eq!(row.get::<NaiveDate, _>(1), march_period().start());
    assert_eq!(row.get::<Option<chrono::DateTime<chrono::Utc>>, _>(2), None);
    assert_eq!(row.get::<Option<String>, _>(3), None);
    assert_eq!(row.get::<Option<String>, _>(4), None);
}

#[sqlx::test]
async fn create_standing_pay_item_refuses_an_effective_from_that_is_not_a_period_start(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;

    let result = create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(50_000),
        date(2026, 2, 1),
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::from(
            PayrollError::EffectiveFromNotAPeriodStart {
                next_valid_effective_from: date(2026, 2, 26),
            }
        ))
    );

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM standing_pay_item")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "a refused create writes nothing");
}

// ---- Proposal at Ordinary run creation ----

#[sqlx::test]
async fn an_ordinary_run_proposes_every_standing_item_in_force(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;

    let allowance_id = create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(60_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();
    let medical_aid_id = create_standing_pay_item(
        &db,
        &employment_id,
        standing_medical_aid(15_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();

    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();

    let rows: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "SELECT source, pay_line_json::text, standing_pay_item_id::text
         FROM payroll_run_pay_line
         WHERE payroll_run_id = $1::uuid AND employment_id = $2
         ORDER BY line",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(rows.len(), 2);
    for (source, _, _) in &rows {
        assert_eq!(source, "standing");
    }
    let ids: Vec<Option<String>> = rows.iter().map(|(_, _, id)| id.clone()).collect();
    assert_eq!(
        ids,
        vec![
            Some(allowance_id.as_str().to_string()),
            Some(medical_aid_id.as_str().to_string())
        ]
    );

    let detail = get_payroll_run_detail(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    assert_eq!(
        detail.members[0].pay_lines[0].standing_effective_from,
        Some(march_period().start())
    );
}

#[sqlx::test]
async fn a_standing_item_effective_after_the_run_period_is_not_proposed(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;

    create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(60_000),
        april_period().start(),
        "actor",
    )
    .await
    .unwrap();

    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payroll_run_pay_line WHERE payroll_run_id = $1::uuid",
    )
    .bind(run_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test]
async fn an_ended_standing_item_is_not_proposed(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;

    let item_id = create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(60_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();
    end_standing_pay_item(&db, &item_id, "no longer applies", "actor")
        .await
        .unwrap();

    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payroll_run_pay_line WHERE payroll_run_id = $1::uuid",
    )
    .bind(run_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test]
async fn creating_a_second_ordinary_run_for_the_same_period_is_refused_and_duplicates_no_standing_line(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;

    create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(60_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();

    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();

    let retry =
        create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 1), "actor")
            .await;
    assert!(
        retry.is_err(),
        "a second Ordinary run for the same period must be refused"
    );

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payroll_run_pay_line
         WHERE payroll_run_id = $1::uuid AND standing_pay_item_id IS NOT NULL",
    )
    .bind(run_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "the retry never touched the first run's own lines"
    );
}

#[sqlx::test]
async fn a_one_off_line_typed_on_one_run_does_not_appear_in_the_next_run(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;

    let first_run =
        create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();
    set_run_pay_lines(
        &db,
        &first_run,
        &employment_id,
        vec![allowance(10_000)],
        Vec::new(),
    )
    .await
    .unwrap();

    let second_run =
        create_ordinary_payroll_run(&db, &employer_id, april_period(), date(2026, 4, 1), "actor")
            .await
            .unwrap();

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payroll_run_pay_line WHERE payroll_run_id = $1::uuid",
    )
    .bind(second_run.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test]
async fn a_correction_run_proposes_no_standing_items(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;

    create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(60_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();

    let correction_run = create_correction_run(
        &db,
        &employer_id,
        march_period(),
        date(2026, 3, 1),
        "correcting",
        "actor",
    )
    .await
    .unwrap();
    add_employment_to_correction_run(&db, &correction_run, &employment_id, None, "actor")
        .await
        .unwrap();

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payroll_run_pay_line WHERE payroll_run_id = $1::uuid",
    )
    .bind(correction_run.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

// ---- EndStandingPayItem ----

#[sqlx::test]
async fn ending_a_standing_item_states_a_reason_and_does_not_delete_the_row(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;

    let item_id = create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(60_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();

    end_standing_pay_item(&db, &item_id, "employee opted out", "actor")
        .await
        .unwrap();

    let row = sqlx::query(
        "SELECT ended_at IS NOT NULL, ended_by, ended_reason
         FROM standing_pay_item WHERE id = $1::uuid",
    )
    .bind(item_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(row.get::<bool, _>(0));
    assert_eq!(row.get::<String, _>(1), "actor");
    assert_eq!(row.get::<String, _>(2), "employee opted out");
}

#[sqlx::test]
async fn ending_a_standing_item_refuses_a_blank_reason(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;

    let item_id = create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(60_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();

    let result = end_standing_pay_item(&db, &item_id, "   ", "actor").await;
    assert_eq!(
        result,
        Err(PayrollAppError::StandingPayItemEndReasonCannotBeEmpty)
    );
}

#[sqlx::test]
async fn ending_an_already_ended_standing_item_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;

    let item_id = create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(60_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();
    end_standing_pay_item(&db, &item_id, "first reason", "actor")
        .await
        .unwrap();

    let result = end_standing_pay_item(&db, &item_id, "second reason", "actor").await;
    assert_eq!(
        result,
        Err(PayrollAppError::StandingPayItemAlreadyEnded(
            item_id.clone()
        ))
    );
}

#[sqlx::test]
async fn ending_a_standing_item_waits_for_the_ordinary_runs_employer_lock(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;
    let item_id = create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(60_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();

    // `create_ordinary_payroll_run` holds this exact row `FOR UPDATE` while
    // it reads active standing items. An end must wait on it, otherwise it
    // could commit between the read and the run's own commit.
    let mut ordinary_run_tx = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM employer WHERE id = $1 FOR UPDATE")
        .bind(employer_id.as_str())
        .execute(&mut *ordinary_run_tx)
        .await
        .unwrap();

    let ending_pool = pool.clone();
    let ending_item_id = item_id.clone();
    let mut ending = tokio::spawn(async move {
        let ending_db = SaltDatabase::from_pool(ending_pool);
        end_standing_pay_item(&ending_db, &ending_item_id, "no longer applies", "actor").await
    });

    tokio::task::yield_now().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut ending)
            .await
            .is_err(),
        "ending must wait for the Employer lock that guards an Ordinary-run proposal"
    );

    ordinary_run_tx.commit().await.unwrap();
    ending.await.unwrap().unwrap();
}

// ---- set_run_pay_lines interaction ----

#[sqlx::test]
async fn set_run_pay_lines_preserves_an_untouched_standing_lines_provenance(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;

    let item_id = create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(60_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();

    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();

    // Resubmits the standing allowance untouched, alongside a new one-off.
    set_run_pay_lines(
        &db,
        &run_id,
        &employment_id,
        vec![allowance(60_000), allowance(5_000)],
        Vec::new(),
    )
    .await
    .unwrap();

    let rows: Vec<(String, Option<String>, i64)> = sqlx::query_as(
        "SELECT source, standing_pay_item_id::text, (pay_line_json->'Earning'->'TaxableAllowance'->'amount')::bigint
         FROM payroll_run_pay_line
         WHERE payroll_run_id = $1::uuid AND employment_id = $2
         ORDER BY line",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0],
        (
            "standing".to_string(),
            Some(item_id.as_str().to_string()),
            60_000
        )
    );
    assert_eq!(rows[1], ("one_off".to_string(), None, 5_000));
}

#[sqlx::test]
async fn set_run_pay_lines_downgrades_an_edited_standing_line_to_one_off(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;

    create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(60_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();

    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();

    // The amount no longer matches the standing item's own instruction.
    set_run_pay_lines(
        &db,
        &run_id,
        &employment_id,
        vec![allowance(30_000)],
        Vec::new(),
    )
    .await
    .unwrap();

    let row: (String, Option<String>) = sqlx::query_as(
        "SELECT source, standing_pay_item_id::text FROM payroll_run_pay_line
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, ("one_off".to_string(), None));
}
