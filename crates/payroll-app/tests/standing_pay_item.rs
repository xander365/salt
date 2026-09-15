//! Proves the use cases issue #79 introduces: `create_standing_pay_item`,
//! `end_standing_pay_item`, and `create_ordinary_payroll_run`'s own new
//! behaviour of proposing every `StandingPayItem` in force as a `standing`
//! pay line (`docs/domain/payroll-run-persistence.md` §0's own words for
//! `StandingPayItem`).

use std::time::Duration;

use chrono::NaiveDate;
use payroll::{
    DayOfMonth, EarningInstruction, EarningLabel, Money, PayPeriod, PayrollError, PeriodEndDay,
    PriorEmployment, TaxYear, UnsupportedDeductionStatus, VoluntaryDeductionInstruction,
};
use payroll_app::{
    EarningPrePopulation, EmploymentPerson, PayLineInstruction, PayLineSource, PayrollAppError,
    SaltDatabase, StandingPayItemInstruction, add_employment_to_correction_run,
    calculate_payroll_run, create_correction_run, create_employer, create_employment,
    create_ordinary_payroll_run, create_standing_pay_item, declare_prior_employment,
    declare_unsupported_deduction_status, end_standing_pay_item, finalize_payroll_run,
    get_payroll_run_detail, list_standing_pay_items, record_compensation_terms,
    reverse_finalized_payroll, set_run_pay_lines, void_employment,
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

fn label() -> EarningLabel {
    EarningLabel::new("standby").unwrap()
}

fn allowance(cents: i64) -> EarningInstruction {
    EarningInstruction::TaxableAllowance {
        amount: Money::from_cents(cents).unwrap(),
        label: Some(label()),
    }
}

fn premium(cents: i64) -> VoluntaryDeductionInstruction {
    VoluntaryDeductionInstruction::MedicalAidPremium(Money::from_cents(cents).unwrap())
}

fn standing_allowance(cents: i64) -> StandingPayItemInstruction {
    StandingPayItemInstruction::TaxableAllowance {
        amount: Money::from_cents(cents).unwrap(),
        label: label(),
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

/// Leave payout, notice pay and severance are refused by name (issue #81,
/// D23), even as a recurring standing item — the worse loophole a one-off
/// refusal alone would leave open.
#[sqlx::test]
async fn create_standing_pay_item_refuses_an_allowance_labelled_severance(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;

    let result = create_standing_pay_item(
        &db,
        &employment_id,
        StandingPayItemInstruction::TaxableAllowance {
            amount: Money::from_cents(50_000).unwrap(),
            label: EarningLabel::new("Severance").unwrap(),
        },
        march_period().start(),
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::EarningLabelIsOutOfScope {
            label: "Severance".to_string(),
        })
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
    end_standing_pay_item(
        &db,
        &employment_id,
        item_id.as_str(),
        "no longer applies",
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
        matches!(retry, Err(PayrollAppError::Database(ref message)) if message.contains("one_ordinary_payroll_run_per_employer_and_period")),
        "a second Ordinary run for the same period must be refused, got {retry:?}"
    );
    let runs: i64 = sqlx::query_scalar("SELECT count(*) FROM payroll_run")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(runs, 1, "the refused retry created no second run");

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

    end_standing_pay_item(
        &db,
        &employment_id,
        item_id.as_str(),
        "employee opted out",
        "actor",
    )
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

    let result = end_standing_pay_item(&db, &employment_id, item_id.as_str(), "   ", "actor").await;
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
    end_standing_pay_item(
        &db,
        &employment_id,
        item_id.as_str(),
        "first reason",
        "actor",
    )
    .await
    .unwrap();

    let result = end_standing_pay_item(
        &db,
        &employment_id,
        item_id.as_str(),
        "second reason",
        "actor",
    )
    .await;
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
    let ending_employment_id = employment_id.clone();
    let mut ending = tokio::spawn(async move {
        let ending_db = SaltDatabase::from_pool(ending_pool);
        end_standing_pay_item(
            &ending_db,
            &ending_employment_id,
            ending_item_id.as_str(),
            "no longer applies",
            "actor",
        )
        .await
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

/// Since issue #80 (decision 2): a body that edits an active standing line
/// without going through an override is refused, not silently downgraded to
/// `one_off` — that downgrade used to make the item look "not proposed", so
/// a later Refresh would add it again and double the payment.
#[sqlx::test]
async fn set_run_pay_lines_refuses_to_edit_or_drop_a_standing_line(pool: PgPool) {
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

    // The amount no longer matches the standing item's own instruction.
    let result = set_run_pay_lines(
        &db,
        &run_id,
        &employment_id,
        vec![allowance(30_000)],
        Vec::new(),
    )
    .await;
    assert_eq!(
        result,
        Err(PayrollAppError::StandingPayLineChangedWithoutOverride {
            payroll_run_id: run_id.clone(),
            employment_id: employment_id.clone(),
            standing_pay_item_id: item_id.clone(),
        })
    );

    let row: (String, Option<String>) = sqlx::query_as(
        "SELECT source, standing_pay_item_id::text FROM payroll_run_pay_line
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        row,
        ("standing".to_string(), Some(item_id.as_str().to_string()))
    );
}

// ---- Hardening (issue #79) ----

/// An Employment with every fact `calculate` needs, starting on `start`, paid
/// N$15,000.00 from the first period start on or before it.
async fn a_payable_employment(
    db: &SaltDatabase,
    employer_id: &payroll::EmployerId,
    start: NaiveDate,
    terms_from: NaiveDate,
) -> payroll::EmploymentId {
    let (_, employment_id) = create_employment(
        db,
        employer_id,
        EmploymentPerson::New("Person".to_string()),
        start,
        None,
        "actor",
    )
    .await
    .unwrap();
    record_compensation_terms(
        db,
        &employment_id,
        terms_from,
        Money::from_cents(1_500_000).unwrap(),
        payroll::OrdinaryHours::new(rust_decimal::Decimal::new(4_000, 2)).unwrap(),
        &[],
        "",
        "actor",
    )
    .await
    .unwrap();
    declare_prior_employment(
        db,
        &employment_id,
        TaxYear::starting(2025),
        PriorEmployment::None,
        "actor",
    )
    .await
    .unwrap();
    declare_unsupported_deduction_status(
        db,
        &employment_id,
        terms_from,
        UnsupportedDeductionStatus::ConfirmedNone,
        &[],
        "a reason",
        "actor",
    )
    .await
    .unwrap();
    employment_id
}

/// `(source, standing_pay_item_id, xmin)` for every stored line, in order.
/// `xmin` is the id of the transaction that last wrote the row, so an
/// unchanged value proves a call wrote nothing at all.
async fn stored_lines(
    pool: &PgPool,
    run_id: &payroll_app::PayrollRunId,
) -> Vec<(String, Option<String>, String)> {
    sqlx::query_as(
        "SELECT source, standing_pay_item_id::text, xmin::text FROM payroll_run_pay_line
         WHERE payroll_run_id = $1::uuid ORDER BY employment_id, line",
    )
    .bind(run_id.as_str())
    .fetch_all(pool)
    .await
    .unwrap()
}

/// A premium recorded before an allowance is still proposed after it:
/// earnings first, then deductions, the one order `set_run_pay_lines`
/// stores. Otherwise the worksheet's unchanged resave would read as a change
/// and retire true figures for nothing.
#[sqlx::test]
async fn standing_lines_are_proposed_earnings_first_so_an_unchanged_resave_writes_nothing(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;
    let premium_id = create_standing_pay_item(
        &db,
        &employment_id,
        standing_medical_aid(15_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();
    let allowance_id = create_standing_pay_item(
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
    let proposed = stored_lines(&pool, &run_id).await;
    let ids: Vec<Option<String>> = proposed.iter().map(|(_, id, _)| id.clone()).collect();
    assert_eq!(
        ids,
        [Some(allowance_id.to_string()), Some(premium_id.to_string())]
    );

    set_run_pay_lines(
        &db,
        &run_id,
        &employment_id,
        vec![allowance(60_000)],
        vec![premium(15_000)],
    )
    .await
    .unwrap();

    assert_eq!(
        stored_lines(&pool, &run_id).await,
        proposed,
        "resaving the proposal unchanged writes nothing"
    );
}

/// Both kinds of proposed line say they are standing, and since when.
#[sqlx::test]
async fn a_proposed_allowance_and_premium_each_name_their_item_and_start(pool: PgPool) {
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
    let premium_id = create_standing_pay_item(
        &db,
        &employment_id,
        standing_medical_aid(15_000),
        april_period().start(),
        "actor",
    )
    .await
    .unwrap();

    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, april_period(), date(2026, 4, 1), "actor")
            .await
            .unwrap();
    let detail = get_payroll_run_detail(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    let lines = &detail.members[0].pay_lines;

    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[0].instruction,
        PayLineInstruction::Earning(allowance(60_000))
    );
    assert_eq!(lines[0].source, PayLineSource::Standing);
    assert_eq!(lines[0].standing_pay_item_id, Some(allowance_id));
    assert_eq!(
        lines[0].standing_effective_from,
        Some(march_period().start())
    );
    assert_eq!(
        lines[1].instruction,
        PayLineInstruction::Deduction(premium(15_000))
    );
    assert_eq!(lines[1].source, PayLineSource::Standing);
    assert_eq!(lines[1].standing_pay_item_id, Some(premium_id));
    assert_eq!(
        lines[1].standing_effective_from,
        Some(april_period().start())
    );
}

#[sqlx::test]
async fn a_zero_standing_medical_aid_premium_is_refused_and_nothing_is_written(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;

    let result = create_standing_pay_item(
        &db,
        &employment_id,
        standing_medical_aid(0),
        march_period().start(),
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::StandingMedicalAidPremiumIsZero)
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM standing_pay_item")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test]
async fn a_standing_item_on_a_void_or_missing_employment_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;
    void_employment(&db, &employment_id, "actor").await.unwrap();

    let void = create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(60_000),
        march_period().start(),
        "actor",
    )
    .await;
    assert_eq!(
        void,
        Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()))
    );

    let missing_id = payroll::EmploymentId::new("no-such-employment");
    let missing = create_standing_pay_item(
        &db,
        &missing_id,
        standing_allowance(60_000),
        march_period().start(),
        "actor",
    )
    .await;
    assert_eq!(
        missing,
        Err(PayrollAppError::EmploymentNotFound(missing_id))
    );
}

/// An item is ended only through the Employment that carries it. Another
/// Employment's id, or an id that is not a UUID, is refused as not found and
/// the item stays in force.
#[sqlx::test]
async fn ending_a_standing_item_through_another_employment_is_refused_as_not_found(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;
    let other_employment_id = an_employment(&db, &employer_id).await;
    let item_id = create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(60_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();

    let through_other = end_standing_pay_item(
        &db,
        &other_employment_id,
        item_id.as_str(),
        "a reason",
        "actor",
    )
    .await;
    assert!(
        matches!(through_other, Err(PayrollAppError::StandingPayItemNotFound(ref id)) if id == &item_id),
        "got {through_other:?}"
    );

    let not_a_uuid =
        end_standing_pay_item(&db, &employment_id, "not-a-uuid", "a reason", "actor").await;
    assert!(
        matches!(not_a_uuid, Err(PayrollAppError::StandingPayItemNotFound(_))),
        "got {not_a_uuid:?}"
    );

    let items = list_standing_pay_items(&db, &employment_id).await.unwrap();
    assert_eq!(items[0].ended, None, "the item is still in force");
}

/// Ended items stay listed with who ended them and why: a past proposal
/// still points at them.
#[sqlx::test]
async fn listing_standing_items_keeps_an_ended_item_with_its_reason(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;
    let ended_id = create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(60_000),
        march_period().start(),
        "creator",
    )
    .await
    .unwrap();
    let in_force_id = create_standing_pay_item(
        &db,
        &employment_id,
        standing_medical_aid(15_000),
        april_period().start(),
        "creator",
    )
    .await
    .unwrap();
    end_standing_pay_item(&db, &employment_id, ended_id.as_str(), "opted out", "ender")
        .await
        .unwrap();

    let items = list_standing_pay_items(&db, &employment_id).await.unwrap();

    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, ended_id);
    assert_eq!(items[0].instruction, standing_allowance(60_000));
    assert_eq!(items[0].effective_from, march_period().start());
    assert_eq!(items[0].created_by, "creator");
    let ending = items[0].ended.as_ref().expect("the first item is ended");
    assert_eq!(ending.ended_by, "ender");
    assert_eq!(ending.reason, "opted out");
    assert_eq!(items[1].id, in_force_id);
    assert_eq!(items[1].instruction, standing_medical_aid(15_000));
    assert_eq!(items[1].ended, None);
}

/// A creation that fails after proposing leaves nothing behind, so the
/// retry proposes each item exactly once. The failure is injected by a
/// trigger that refuses the first membership write it sees after a pay
/// line, the point where a real failure would hurt most.
#[sqlx::test]
async fn retrying_a_failed_run_creation_proposes_each_standing_item_once(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let first = an_employment(&db, &employer_id).await;
    let second = an_employment(&db, &employer_id).await;
    for employment_id in [&first, &second] {
        create_standing_pay_item(
            &db,
            employment_id,
            standing_allowance(60_000),
            march_period().start(),
            "actor",
        )
        .await
        .unwrap();
    }

    sqlx::query("CREATE TABLE fail_once (armed BOOLEAN NOT NULL)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO fail_once VALUES (TRUE)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "CREATE FUNCTION fail_after_a_proposed_line() RETURNS trigger AS $$
         BEGIN
             IF EXISTS (SELECT 1 FROM fail_once WHERE armed)
                AND EXISTS (SELECT 1 FROM payroll_run_pay_line WHERE payroll_run_id = NEW.payroll_run_id)
             THEN
                 RAISE EXCEPTION 'injected failure';
             END IF;
             RETURN NEW;
         END $$ LANGUAGE plpgsql",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_after_a_proposed_line BEFORE INSERT ON payroll_run_employment
         FOR EACH ROW EXECUTE FUNCTION fail_after_a_proposed_line()",
    )
    .execute(&pool)
    .await
    .unwrap();

    let failed =
        create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 1), "actor")
            .await;
    assert!(
        matches!(failed, Err(PayrollAppError::Database(ref message)) if message.contains("injected failure")),
        "got {failed:?}"
    );
    let left_behind: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM payroll_run) + (SELECT count(*) FROM payroll_run_pay_line)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(left_behind, 0, "the failed creation left nothing behind");

    // Whatever caused the failure is gone; the Operator tries again.
    sqlx::query("UPDATE fail_once SET armed = FALSE")
        .execute(&pool)
        .await
        .unwrap();
    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();

    let per_item: Vec<i64> = sqlx::query_scalar(
        "SELECT count(*) FROM payroll_run_pay_line
         WHERE payroll_run_id = $1::uuid GROUP BY standing_pay_item_id ORDER BY 1",
    )
    .bind(run_id.as_str())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(per_item, [1, 1]);
}

/// The guarantee behind every retry is structural: the database itself
/// refuses a second line for one StandingPayItem on one member of one run.
#[sqlx::test]
async fn the_database_refuses_a_second_line_for_one_standing_item(pool: PgPool) {
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

    let err = sqlx::query(
        "INSERT INTO payroll_run_pay_line
            (payroll_run_id, employment_id, line, pay_line_json, source, standing_pay_item_id)
         SELECT payroll_run_id, employment_id, line + 1, pay_line_json, source, standing_pay_item_id
         FROM payroll_run_pay_line WHERE payroll_run_id = $1::uuid",
    )
    .bind(run_id.as_str())
    .execute(&pool)
    .await
    .expect_err("a duplicate standing line is refused");

    let sqlx::Error::Database(db_err) = &err else {
        panic!("expected a database refusal, got {err:?}");
    };
    assert_eq!(db_err.code().as_deref(), Some("23505"));
    assert_eq!(
        db_err.constraint(),
        Some("payroll_run_pay_line_one_line_per_standing_item")
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payroll_run_pay_line WHERE standing_pay_item_id = $1::uuid",
    )
    .bind(item_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

/// Only BasicPay is prorated. A joiner's part month proposes the standing
/// allowance and premium at their full amounts, calculates with them whole,
/// and is flagged so the worksheet can say so beside them.
#[sqlx::test]
async fn salt_policy_a_joiners_standing_items_are_proposed_and_calculated_at_full_amount(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    // Joins on 2026-02-10, part way through the 2026-01-26..2026-02-25 period.
    let joiner =
        a_payable_employment(&db, &employer_id, date(2026, 2, 10), march_period().start()).await;
    let continuing =
        a_payable_employment(&db, &employer_id, date(2025, 1, 1), date(2025, 1, 26)).await;
    for employment_id in [&joiner, &continuing] {
        create_standing_pay_item(
            &db,
            employment_id,
            standing_allowance(60_000),
            march_period().start(),
            "actor",
        )
        .await
        .unwrap();
        create_standing_pay_item(
            &db,
            employment_id,
            standing_medical_aid(15_000),
            march_period().start(),
            "actor",
        )
        .await
        .unwrap();
    }

    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();
    let refusals = calculate_payroll_run(&db, &run_id, "actor").await.unwrap();
    assert!(refusals.is_empty(), "got {refusals:?}");

    let detail = get_payroll_run_detail(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    for member in &detail.members {
        let is_joiner = member.employment_id == joiner;
        assert_eq!(member.basic_pay_prorated, is_joiner);
        assert_eq!(
            member
                .pay_lines
                .iter()
                .map(|line| line.instruction.clone())
                .collect::<Vec<_>>(),
            [
                PayLineInstruction::Earning(allowance(60_000)),
                PayLineInstruction::Deduction(premium(15_000)),
            ]
        );
        let figures = member.figures.expect("the member calculated");
        assert_eq!(
            figures.taxable_allowances,
            Money::from_cents(60_000).unwrap()
        );
        assert_eq!(
            figures.medical_aid_premium,
            Money::from_cents(15_000).unwrap()
        );
        if is_joiner {
            assert!(figures.basic_pay < Money::from_cents(1_500_000).unwrap());
        } else {
            assert_eq!(figures.basic_pay, Money::from_cents(1_500_000).unwrap());
        }
    }
}

/// A CorrectionRun's lines come from the reversed payroll's frozen snapshot,
/// never from today's standing records — even when the item it was proposed
/// from has since ended and a different one is in force.
#[sqlx::test]
async fn a_correction_run_copies_the_reversed_snapshot_and_never_todays_standing_items(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    // Starts with this period, so it has no earlier period to finalize first.
    let employment_id = a_payable_employment(
        &db,
        &employer_id,
        march_period().start(),
        march_period().start(),
    )
    .await;
    let original_id = create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(60_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();
    let ordinary_run_id =
        create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();
    calculate_payroll_run(&db, &ordinary_run_id, "actor")
        .await
        .unwrap();
    let outcome = finalize_payroll_run(&db, &ordinary_run_id, "actor")
        .await
        .unwrap();
    let target = outcome.finalized[0].1.clone();
    reverse_finalized_payroll(&db, &target, "allowance was wrong", "actor")
        .await
        .unwrap();

    // Today's standing records now disagree with history.
    end_standing_pay_item(&db, &employment_id, original_id.as_str(), "raised", "actor")
        .await
        .unwrap();
    create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(90_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();
    create_standing_pay_item(
        &db,
        &employment_id,
        standing_medical_aid(15_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();

    let correction_run_id = create_correction_run(
        &db,
        &employer_id,
        march_period(),
        date(2026, 6, 5),
        "allowance was wrong",
        "actor",
    )
    .await
    .unwrap();
    let pre_population = add_employment_to_correction_run(
        &db,
        &correction_run_id,
        &employment_id,
        Some(&target),
        "actor",
    )
    .await
    .unwrap();
    assert_eq!(
        pre_population,
        EarningPrePopulation::FromTarget { count: 1 }
    );

    let detail = get_payroll_run_detail(&db, &employer_id, correction_run_id.as_str())
        .await
        .unwrap();
    let lines = &detail.members[0].pay_lines;
    assert_eq!(lines.len(), 1);
    assert_eq!(
        lines[0].instruction,
        PayLineInstruction::Earning(allowance(60_000))
    );
    assert_eq!(lines[0].source, PayLineSource::FromReversedSnapshot);
    assert_eq!(lines[0].standing_pay_item_id, None);
    assert_eq!(lines[0].standing_effective_from, None);
}
