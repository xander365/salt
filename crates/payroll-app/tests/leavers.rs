//! Proves the use case issue #81 introduces: `record_employment_end_date` —
//! an operator records that someone has left, with a stated reason, and the
//! write is refused outright rather than acknowledged-and-proceeded with
//! when it would invalidate paid history.

use std::time::Duration;

use chrono::NaiveDate;
use payroll::{
    EarningInstruction, EarningLabel, EmployerId, EmploymentId, Money, PayPeriod, PeriodEndDay,
    PriorEmployment, TaxYear, UnsupportedDeductionStatus,
};
use payroll_app::{
    EmploymentPerson, PayLineInstruction, PayrollAppError, PayrollRunId, SaltDatabase,
    StandingPayItemInstruction, calculate_payroll_run, create_employer, create_employment,
    create_ordinary_payroll_run, create_standing_pay_item, declare_prior_employment,
    declare_unsupported_deduction_status, finalize_payroll_run, get_employment_detail,
    get_payroll_run_detail, record_compensation_terms, record_employment_end_date, void_employment,
};
use sqlx::{PgPool, Row};

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn monthly_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::LastDayOfMonth)
}

/// March 2026 — the calendar month `monthly_schedule()` generates.
fn march() -> PayPeriod {
    PayPeriod::new(date(2026, 3, 1), date(2026, 3, 31)).unwrap()
}

/// The PayPeriod immediately after `march()`.
fn april() -> PayPeriod {
    PayPeriod::new(date(2026, 4, 1), date(2026, 4, 30)).unwrap()
}

/// The PayPeriod immediately after `april()`.
fn may() -> PayPeriod {
    PayPeriod::new(date(2026, 5, 1), date(2026, 5, 31)).unwrap()
}

async fn an_employer(db: &SaltDatabase) -> EmployerId {
    create_employer(db, "Employer", monthly_schedule(), "actor")
        .await
        .unwrap()
}

/// An Employment starting on `march()`'s own first day, with every fact
/// `calculate` needs already on record — mirrors
/// `finalize_payroll_run.rs`'s own `a_fully_declared_employment`.
async fn a_fully_declared_employment(db: &SaltDatabase, employer_id: &EmployerId) -> EmploymentId {
    let (_, employment_id) = create_employment(
        db,
        employer_id,
        EmploymentPerson::New("Ada Lovelace".to_string()),
        march().start(),
        None,
        "actor",
    )
    .await
    .unwrap();
    record_compensation_terms(
        db,
        &employment_id,
        march().start(),
        Money::from_cents(310_000).unwrap(),
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
        TaxYear::for_period_end(march().end()),
        PriorEmployment::None,
        "actor",
    )
    .await
    .unwrap();
    declare_unsupported_deduction_status(
        db,
        &employment_id,
        march().start(),
        UnsupportedDeductionStatus::ConfirmedNone,
        &[],
        "a reason",
        "actor",
    )
    .await
    .unwrap();
    employment_id
}

async fn a_live_finalized_march(
    db: &SaltDatabase,
    employer_id: &EmployerId,
) -> (EmploymentId, PayrollRunId) {
    let employment_id = a_fully_declared_employment(db, employer_id).await;
    let run_id = create_ordinary_payroll_run(db, employer_id, march(), date(2026, 4, 5), "actor")
        .await
        .unwrap();
    let refusals = calculate_payroll_run(db, &run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(refusals, Vec::new(), "the run must reach Calculated");
    finalize_payroll_run(db, &run_id, "finalizer")
        .await
        .unwrap();
    (employment_id, run_id)
}

// ---- RecordEmploymentEndDate ----

#[sqlx::test]
async fn recording_an_end_date_with_a_reason_writes_it_and_an_action_log_entry(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let (_, employment_id) = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::New("Ada Lovelace".to_string()),
        date(2026, 1, 1),
        None,
        "actor",
    )
    .await
    .unwrap();

    record_employment_end_date(
        &db,
        &employer_id,
        &employment_id,
        date(2026, 6, 15),
        "  resigned  ",
        "operator:alice",
    )
    .await
    .unwrap();

    let detail = get_employment_detail(&db, &employer_id, &employment_id, date(2026, 7, 1))
        .await
        .unwrap();
    assert_eq!(detail.end_date, Some(date(2026, 6, 15)));

    let row = sqlx::query(
        "SELECT actor, action_type, context FROM action_log_entry WHERE target_id = $1",
    )
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>(0), "operator:alice");
    assert_eq!(row.get::<String, _>(1), "employment_end_date_recorded");
    let context: serde_json::Value = row.get(2);
    assert_eq!(context["reason"], "resigned");
    assert_eq!(context["before"]["end_date"], serde_json::Value::Null);
    assert_eq!(context["after"]["end_date"], "2026-06-15");
}

#[sqlx::test]
async fn the_largest_supported_date_is_recorded_without_panicking(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let (_, employment_id) = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::New("Ada Lovelace".to_string()),
        date(2026, 1, 1),
        None,
        "actor",
    )
    .await
    .unwrap();

    record_employment_end_date(
        &db,
        &employer_id,
        &employment_id,
        NaiveDate::MAX,
        "contract ended",
        "operator:alice",
    )
    .await
    .unwrap();

    let stored: NaiveDate = sqlx::query_scalar("SELECT end_date FROM employment WHERE id = $1")
        .bind(employment_id.as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, NaiveDate::MAX);
}

#[sqlx::test]
async fn a_blank_reason_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let (_, employment_id) = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::New("Ada Lovelace".to_string()),
        date(2026, 1, 1),
        None,
        "actor",
    )
    .await
    .unwrap();

    let err = record_employment_end_date(
        &db,
        &employer_id,
        &employment_id,
        date(2026, 6, 15),
        "   ",
        "operator:alice",
    )
    .await
    .unwrap_err();

    assert_eq!(err, PayrollAppError::EmploymentEndDateReasonCannotBeEmpty);
}

#[sqlx::test]
async fn an_end_date_before_the_start_date_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let (_, employment_id) = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::New("Ada Lovelace".to_string()),
        date(2026, 3, 10),
        None,
        "actor",
    )
    .await
    .unwrap();

    let err = record_employment_end_date(
        &db,
        &employer_id,
        &employment_id,
        date(2026, 3, 1),
        "resigned",
        "operator:alice",
    )
    .await
    .unwrap_err();

    assert_eq!(
        err,
        PayrollAppError::EmploymentEndsBeforeItStarts {
            start_date: date(2026, 3, 10),
            end_date: date(2026, 3, 1),
        }
    );
}

/// The headline refusal (issue #81's own acceptance criterion): an end date
/// falling before a period already paid for the Employment is refused,
/// naming that period, rather than silently accepted or merely warned about.
#[sqlx::test]
async fn an_end_date_before_an_already_paid_period_is_refused_naming_it(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let (employment_id, _run_id) = a_live_finalized_march(&db, &employer_id).await;

    let err = record_employment_end_date(
        &db,
        &employer_id,
        &employment_id,
        date(2026, 3, 15),
        "resigned",
        "operator:alice",
    )
    .await
    .unwrap_err();

    assert_eq!(
        err,
        PayrollAppError::EmploymentEndDatePrecedesPaidPeriods {
            employment_id: employment_id.clone(),
            end_date: date(2026, 3, 15),
            periods: vec![march()],
        }
    );

    let detail = get_employment_detail(&db, &employer_id, &employment_id, date(2026, 4, 1))
        .await
        .unwrap();
    assert_eq!(detail.end_date, None, "a refused write changes nothing");
}

/// Leaving on the paid period's own last day is a no-op for proration
/// (§8.2's own "leaving on the period's last day makes proration a no-op"):
/// it must not be refused as preceding that same period.
#[sqlx::test]
async fn an_end_date_on_the_last_day_of_an_already_paid_period_is_accepted(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let (employment_id, _run_id) = a_live_finalized_march(&db, &employer_id).await;

    record_employment_end_date(
        &db,
        &employer_id,
        &employment_id,
        march().end(),
        "resigned",
        "operator:alice",
    )
    .await
    .unwrap();

    let detail = get_employment_detail(&db, &employer_id, &employment_id, date(2026, 4, 1))
        .await
        .unwrap();
    assert_eq!(detail.end_date, Some(march().end()));
}

/// An end date falling inside April makes the leaver a member of April's own
/// Ordinary run (§8.2's own "the leaver is a member of the Ordinary run for
/// the period their end date falls in"), prorated by employed calendar days
/// — and never a member of any run after that, with no preceding-period
/// block on May's own finalization (§8.2, `sequencing.rs`'s resolved-period
/// rule already excludes an ended Employment; nothing here changes it).
#[sqlx::test]
async fn a_leaver_is_paid_the_period_their_end_date_falls_in_and_never_proposed_again(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let (employment_id, _run_id) = a_live_finalized_march(&db, &employer_id).await;

    record_employment_end_date(
        &db,
        &employer_id,
        &employment_id,
        date(2026, 4, 15),
        "resigned",
        "operator:alice",
    )
    .await
    .unwrap();

    create_standing_pay_item(
        &db,
        &employment_id,
        StandingPayItemInstruction::TaxableAllowance {
            amount: Money::from_cents(60_000).unwrap(),
            label: EarningLabel::new("standby allowance").unwrap(),
        },
        april().start(),
        "actor",
    )
    .await
    .unwrap();

    let april_run_id =
        create_ordinary_payroll_run(&db, &employer_id, april(), date(2026, 5, 5), "actor")
            .await
            .unwrap();
    let april_member_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payroll_run_employment
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(april_run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        april_member_count, 1,
        "the leaver is a member of the Ordinary run for the period their end date falls in"
    );
    let april_refusals = calculate_payroll_run(&db, &april_run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(april_refusals, Vec::new());
    let april_detail = get_payroll_run_detail(&db, &employer_id, april_run_id.as_str())
        .await
        .unwrap();
    let april_member = april_detail
        .members
        .iter()
        .find(|member| member.employment_id == employment_id)
        .expect("the leaver remains a member of their final eligible period");
    assert!(
        april_member.basic_pay_prorated,
        "the worksheet must explain that this member's BasicPay is prorated"
    );
    assert_eq!(
        april_member
            .pay_lines
            .iter()
            .map(|line| line.instruction.clone())
            .collect::<Vec<_>>(),
        [PayLineInstruction::Earning(
            EarningInstruction::TaxableAllowance {
                amount: Money::from_cents(60_000).unwrap(),
                label: Some(EarningLabel::new("standby allowance").unwrap()),
            },
        )],
        "the leaver's standing item is proposed at its full amount"
    );
    let figures = april_member
        .figures
        .as_ref()
        .expect("the leaver calculated");
    assert_eq!(
        figures.basic_pay,
        Money::from_cents(155_000).unwrap(),
        "15 of April's 30 employed calendar days prorates BasicPay exactly"
    );
    assert_eq!(
        figures.taxable_allowances,
        Money::from_cents(60_000).unwrap(),
        "the standing allowance is not prorated with BasicPay"
    );
    finalize_payroll_run(&db, &april_run_id, "finalizer")
        .await
        .unwrap();

    let may_run_id =
        create_ordinary_payroll_run(&db, &employer_id, may(), date(2026, 6, 5), "actor")
            .await
            .unwrap();
    let may_member_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payroll_run_employment
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(may_run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        may_member_count, 0,
        "a leaver is never proposed again once their final period is behind them"
    );

    let may_refusals = calculate_payroll_run(&db, &may_run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(
        may_refusals,
        Vec::new(),
        "the next run must finalize without a preceding-period block"
    );
    finalize_payroll_run(&db, &may_run_id, "finalizer")
        .await
        .unwrap();
}

#[sqlx::test]
async fn recording_an_end_date_waits_for_the_ordinary_runs_membership_lock(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = a_fully_declared_employment(&db, &employer_id).await;

    let mut run_transaction = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM employer WHERE id = $1 FOR UPDATE")
        .bind(employer_id.as_str())
        .execute(&mut *run_transaction)
        .await
        .unwrap();

    let ending_pool = pool.clone();
    let ending_employer_id = employer_id.clone();
    let ending_employment_id = employment_id.clone();
    let mut ending = tokio::spawn(async move {
        let ending_db = SaltDatabase::from_pool(ending_pool);
        record_employment_end_date(
            &ending_db,
            &ending_employer_id,
            &ending_employment_id,
            date(2026, 4, 15),
            "resigned",
            "operator:alice",
        )
        .await
    });

    tokio::task::yield_now().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut ending)
            .await
            .is_err(),
        "recording a leaver must wait for the lock guarding an Ordinary-run membership snapshot"
    );

    run_transaction.commit().await.unwrap();
    ending.await.unwrap().unwrap();
}

#[sqlx::test]
async fn an_employment_id_belonging_to_another_employer_is_refused_like_one_that_does_not_exist(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let other_employer_id = an_employer(&db).await;
    let (_, employment_id) = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::New("Ada Lovelace".to_string()),
        date(2026, 1, 1),
        None,
        "actor",
    )
    .await
    .unwrap();

    let err = record_employment_end_date(
        &db,
        &other_employer_id,
        &employment_id,
        date(2026, 6, 15),
        "resigned",
        "operator:alice",
    )
    .await
    .unwrap_err();

    assert_eq!(err, PayrollAppError::EmploymentNotFound(employment_id));
}

#[sqlx::test]
async fn a_void_employment_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let (_, employment_id) = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::New("Ada Lovelace".to_string()),
        date(2026, 1, 1),
        None,
        "actor",
    )
    .await
    .unwrap();
    void_employment(&db, &employment_id, "actor").await.unwrap();

    let err = record_employment_end_date(
        &db,
        &employer_id,
        &employment_id,
        date(2026, 6, 15),
        "resigned",
        "operator:alice",
    )
    .await
    .unwrap_err();

    assert_eq!(err, PayrollAppError::EmploymentIsVoid(employment_id));
}
