//! Proves ADR-0013 and issue #33: `OpeningBalance` and `PriorEmployment`
//! freeze at that Employment's first finalization in that TaxYear (Live or
//! reversed alike), `CompensationTerms` and `UnsupportedDeductionStatus`
//! stay editable, and `change_pay_schedule` refuses once any payroll is
//! finalized in the given TaxYear. Every fixture reaches a real
//! `FinalizedPayroll` through the public API, exactly as
//! `tests/finalize_payroll_run.rs` and `tests/reverse_finalized_payroll.rs`
//! do.

use chrono::NaiveDate;
use payroll::{
    DayOfMonth, EmployerId, EmploymentId, Money, PayPeriod, PeriodEndDay, PersonId,
    PriorEmployment, PriorEmploymentFigures, TaxYear, UnsupportedDeductionKind,
    UnsupportedDeductionKinds, UnsupportedDeductionStatus,
};
use payroll_app::{
    FinalizedPayrollId, PayrollAppError, calculate_payroll_run, change_pay_schedule,
    create_employer, create_employment, create_ordinary_payroll_run, declare_prior_employment,
    declare_unsupported_deduction_status, finalize_payroll_run, record_compensation_terms,
    record_opening_balance, reverse_finalized_payroll,
};
use sqlx::{PgPool, Row};

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn monthly_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::LastDayOfMonth)
}

fn period() -> PayPeriod {
    PayPeriod::new(date(2026, 3, 1), date(2026, 3, 31)).unwrap()
}

async fn an_employer(pool: &PgPool) -> EmployerId {
    create_employer(pool, monthly_schedule(), "actor")
        .await
        .unwrap()
}

/// An Employment starting on `period()`'s own first day, with `calculate`'s
/// three required facts on record, plus a (zero-figure) `OpeningBalance` at
/// its own first payable period end — the guard-4 case, so it is a
/// legitimate row and not itself refused.
async fn a_fully_declared_employment(
    pool: &PgPool,
    employer_id: &EmployerId,
    person: &str,
    basic_pay: Money,
) -> EmploymentId {
    let employment_id = create_employment(
        pool,
        employer_id,
        &PersonId::new(person),
        period().start(),
        None,
        "actor",
    )
    .await
    .unwrap();
    record_compensation_terms(pool, &employment_id, period().start(), basic_pay, "actor")
        .await
        .unwrap();
    record_opening_balance(
        pool,
        &employment_id,
        TaxYear::for_period_end(period().end()),
        period().end(),
        Money::ZERO,
        Money::ZERO,
        "actor",
    )
    .await
    .unwrap();
    declare_prior_employment(
        pool,
        &employment_id,
        TaxYear::for_period_end(period().end()),
        PriorEmployment::None,
        "actor",
    )
    .await
    .unwrap();
    declare_unsupported_deduction_status(
        pool,
        &employment_id,
        period().start(),
        UnsupportedDeductionStatus::ConfirmedNone,
        "actor",
    )
    .await
    .unwrap();
    employment_id
}

/// Creates, calculates and finalizes a March Ordinary run for `employment_id`
/// alone, returning the `FinalizedPayrollId` it produced.
async fn finalize_march(
    pool: &PgPool,
    employer_id: &EmployerId,
    employment_id: &EmploymentId,
) -> FinalizedPayrollId {
    let run_id =
        create_ordinary_payroll_run(pool, employer_id, period(), date(2026, 4, 5), "actor")
            .await
            .unwrap();
    let refusals = calculate_payroll_run(pool, &run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(refusals, Vec::new(), "the run must reach Calculated");
    let finalized = finalize_payroll_run(pool, &run_id, "finalizer")
        .await
        .unwrap();
    let (_, finalized_payroll_id) = finalized
        .into_iter()
        .find(|(id, _)| id == employment_id)
        .expect("the fully declared Employment must have finalized");
    finalized_payroll_id
}

// ---- OpeningBalance freezes (§4.5, ADR-0013) ----

#[sqlx::test]
async fn opening_balance_is_refused_after_the_employments_first_finalization(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    finalize_march(&pool, &employer_id, &employment_id).await;

    let result = record_opening_balance(
        &pool,
        &employment_id,
        TaxYear::starting(2026),
        period().end(),
        Money::from_cents(1).unwrap(),
        Money::ZERO,
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::OpeningBalanceFrozenByFinalization {
            employment_id: employment_id.clone(),
            tax_year: TaxYear::starting(2026),
        })
    );

    // The pre-finalization row itself must be untouched.
    let row = sqlx::query(
        "SELECT prior_taxable_remuneration FROM opening_balance WHERE employment_id = $1",
    )
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<i64, _>(0), 0);
}

/// A reversal must not thaw a frozen `OpeningBalance` — the exact case
/// ADR-0013's Deep Instructions name: a forgotten period must never become
/// retroactively reclassified as pre-Salt.
#[sqlx::test]
async fn a_reversal_does_not_thaw_a_frozen_opening_balance(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    let finalized_payroll_id = finalize_march(&pool, &employer_id, &employment_id).await;

    reverse_finalized_payroll(
        &pool,
        &finalized_payroll_id,
        "correcting a mistake",
        "actor",
    )
    .await
    .unwrap();

    let result = record_opening_balance(
        &pool,
        &employment_id,
        TaxYear::starting(2026),
        period().end(),
        Money::from_cents(1).unwrap(),
        Money::ZERO,
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::OpeningBalanceFrozenByFinalization {
            employment_id,
            tax_year: TaxYear::starting(2026),
        })
    );
}

/// The freeze is per Employment, not per Employer (§4.5, ADR-0013): a second
/// Employment onboarded later must still accept its own `OpeningBalance`
/// even though the first Employment's March run already finalized.
#[sqlx::test]
async fn the_freeze_is_per_employment_not_per_employer(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let finalized_employment = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    finalize_march(&pool, &employer_id, &finalized_employment).await;

    let june_start = date(2026, 6, 1);
    let june_employment = create_employment(
        &pool,
        &employer_id,
        &PersonId::new("person-2"),
        june_start,
        None,
        "actor",
    )
    .await
    .unwrap();

    // June is this Employment's own first payable period end — an empty
    // covered span, so zero figures are the legitimate boundary here.
    record_opening_balance(
        &pool,
        &june_employment,
        TaxYear::starting(2026),
        date(2026, 6, 30),
        Money::ZERO,
        Money::ZERO,
        "actor",
    )
    .await
    .unwrap();
}

// ---- PriorEmployment freezes (§4.5b, ADR-0013) ----

#[sqlx::test]
async fn prior_employment_is_refused_after_the_employments_first_finalization(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    finalize_march(&pool, &employer_id, &employment_id).await;

    let result = declare_prior_employment(
        &pool,
        &employment_id,
        TaxYear::starting(2026),
        PriorEmployment::Some(PriorEmploymentFigures::new(
            Money::from_cents(50_000).unwrap(),
            Money::ZERO,
        )),
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::PriorEmploymentFrozenByFinalization {
            employment_id: employment_id.clone(),
            tax_year: TaxYear::starting(2026),
        })
    );

    let row =
        sqlx::query("SELECT status FROM prior_employment_declaration WHERE employment_id = $1")
            .bind(employment_id.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.get::<String, _>(0), "confirmed_none");
}

/// A `PriorEmployment` declaration for a different TaxYear must stay
/// writable — the freeze is keyed on (Employment, TaxYear), not the whole
/// Employment.
#[sqlx::test]
async fn prior_employment_for_a_different_tax_year_stays_writable(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    finalize_march(&pool, &employer_id, &employment_id).await;

    declare_prior_employment(
        &pool,
        &employment_id,
        TaxYear::starting(2027),
        PriorEmployment::None,
        "actor",
    )
    .await
    .unwrap();
}

// ---- CompensationTerms and UnsupportedDeductionStatus stay editable ----

#[sqlx::test]
async fn compensation_terms_stays_editable_after_finalization(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    finalize_march(&pool, &employer_id, &employment_id).await;

    // A record-only use case: correcting an existing row's own
    // `effective_from` is a separate, later use case, so a genuinely
    // editable-after-finalization write is a new row, effective from the
    // next period.
    record_compensation_terms(
        &pool,
        &employment_id,
        date(2026, 4, 1),
        Money::from_cents(1_600_000).unwrap(),
        "actor",
    )
    .await
    .unwrap();
}

/// The story ADR-0013 and §4.5c give directly: an employee joining a
/// provident fund in August refuses August onward while March-July stay
/// finalized and untouched, with no reversal required.
#[sqlx::test]
async fn recording_a_present_provident_fund_status_after_finalization_leaves_earlier_periods_untouched(
    pool: PgPool,
) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    finalize_march(&pool, &employer_id, &employment_id).await;

    let kinds =
        UnsupportedDeductionKinds::new(vec![UnsupportedDeductionKind::ProvidentFund]).unwrap();
    declare_unsupported_deduction_status(
        &pool,
        &employment_id,
        date(2026, 8, 1),
        UnsupportedDeductionStatus::Present(kinds),
        "actor",
    )
    .await
    .unwrap();

    // March's own declaration is untouched, and no reversal was required for
    // the already-live March FinalizedPayroll.
    let march_status: String = sqlx::query_scalar(
        "SELECT status FROM unsupported_deduction_declaration
         WHERE employment_id = $1 AND effective_from = $2",
    )
    .bind(employment_id.as_str())
    .bind(period().start())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(march_status, "confirmed_none");

    let live: i64 =
        sqlx::query_scalar("SELECT count(*) FROM live_finalized_payroll WHERE employment_id = $1")
            .bind(employment_id.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(live, 1, "the March FinalizedPayroll must still be live");
}

// ---- PaySchedule change (§4.2, ADR-0013) ----

fn twenty_fifth_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap()))
}

#[sqlx::test]
async fn a_pay_schedule_change_is_refused_once_any_payroll_is_finalized_in_that_tax_year(
    pool: PgPool,
) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    finalize_march(&pool, &employer_id, &employment_id).await;

    let result = change_pay_schedule(
        &pool,
        &employer_id,
        twenty_fifth_schedule(),
        TaxYear::starting(2026),
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::PayScheduleFrozenByFinalization {
            employer_id: employer_id.clone(),
            tax_year: TaxYear::starting(2026),
        })
    );

    let (kind, value): (String, Option<i16>) = sqlx::query_as(
        "SELECT period_end_day_kind, period_end_day_value FROM employer WHERE id = $1",
    )
    .bind(employer_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(kind, "last_day_of_month");
    assert_eq!(value, None);
}

#[sqlx::test]
async fn a_pay_schedule_change_is_allowed_before_any_finalization_in_that_tax_year(pool: PgPool) {
    let employer_id = an_employer(&pool).await;

    change_pay_schedule(
        &pool,
        &employer_id,
        twenty_fifth_schedule(),
        TaxYear::starting(2026),
        "actor",
    )
    .await
    .unwrap();

    let (kind, value): (String, Option<i16>) = sqlx::query_as(
        "SELECT period_end_day_kind, period_end_day_value FROM employer WHERE id = $1",
    )
    .bind(employer_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(kind, "day");
    assert_eq!(value, Some(25));
}

/// A payroll finalized in a *different* TaxYear must not block a change made
/// in the current one — the guard names "the current TaxYear" specifically.
#[sqlx::test]
async fn a_finalization_in_a_different_tax_year_does_not_block_the_change(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    finalize_march(&pool, &employer_id, &employment_id).await;

    change_pay_schedule(
        &pool,
        &employer_id,
        twenty_fifth_schedule(),
        TaxYear::starting(2027),
        "actor",
    )
    .await
    .unwrap();
}

#[sqlx::test]
async fn an_allowed_pay_schedule_change_writes_a_pay_schedule_changed_action_log_entry(
    pool: PgPool,
) {
    let employer_id = an_employer(&pool).await;

    change_pay_schedule(
        &pool,
        &employer_id,
        twenty_fifth_schedule(),
        TaxYear::starting(2026),
        "actor",
    )
    .await
    .unwrap();

    let row = sqlx::query(
        "SELECT employer_id, actor, action_type, target_type, target_id
         FROM action_log_entry WHERE target_id = $1",
    )
    .bind(employer_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>(0), employer_id.as_str());
    assert_eq!(row.get::<String, _>(1), "actor");
    assert_eq!(row.get::<String, _>(2), "pay_schedule_changed");
    assert_eq!(row.get::<String, _>(3), "employer");
}

/// A refused change must write no `ActionLog` entry — only an act that
/// actually happened is logged.
#[sqlx::test]
async fn a_refused_pay_schedule_change_writes_no_action_log_entry(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    finalize_march(&pool, &employer_id, &employment_id).await;

    let _ = change_pay_schedule(
        &pool,
        &employer_id,
        twenty_fifth_schedule(),
        TaxYear::starting(2026),
        "actor",
    )
    .await;

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM action_log_entry WHERE action_type = 'pay_schedule_changed'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test]
async fn changing_the_pay_schedule_of_a_missing_employer_is_refused(pool: PgPool) {
    let missing = EmployerId::new("does-not-exist");

    let result = change_pay_schedule(
        &pool,
        &missing,
        twenty_fifth_schedule(),
        TaxYear::starting(2026),
        "actor",
    )
    .await;

    assert_eq!(result, Err(PayrollAppError::EmployerNotFound(missing)));
}
