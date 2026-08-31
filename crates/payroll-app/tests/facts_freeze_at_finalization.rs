//! Proves ADR-0013 and issue #33: `OpeningBalance` and `PriorEmployment`
//! freeze at that Employment's first finalization in that TaxYear (Live or
//! reversed alike), `CompensationTerms` and `UnsupportedDeductionStatus`
//! stay editable, and `change_pay_schedule` refuses once any payroll is
//! finalized in the given TaxYear or a later one — and, since that TaxYear
//! is only the caller's claim, that a schedule moved under a part-finalized
//! TaxYear stops any further run of that year regardless. Every fixture reaches a real
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
    FinalizedPayrollId, PayrollAppError, ScheduleBoundedFact, calculate_payroll_run,
    change_pay_schedule, create_employer, create_employment, create_ordinary_payroll_run,
    declare_prior_employment, declare_unsupported_deduction_status, finalize_payroll_run,
    record_compensation_terms, record_opening_balance, reverse_finalized_payroll, void_employment,
};
use sqlx::{Acquire, PgPool, Row};
use tokio::sync::oneshot;

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

/// The TaxYear the caller names is a claim, not a fact. Naming an *earlier*
/// one is disprovable from the database alone, so it is refused: the check
/// reads "in that TaxYear or later" (§4.2).
#[sqlx::test]
async fn a_pay_schedule_change_claiming_an_earlier_tax_year_is_refused(pool: PgPool) {
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
        TaxYear::starting(2025),
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::PayScheduleFrozenByFinalization {
            employer_id: employer_id.clone(),
            tax_year: TaxYear::starting(2025),
        })
    );
}

/// Naming a *later* TaxYear cannot be disproved without a clock, so the
/// twelve-period invariant is held where the harm would land instead: the
/// next run of the part-finalized TaxYear is refused, whatever the schedule
/// change claimed (§4.2, ADR-0005).
#[sqlx::test]
async fn a_schedule_moved_under_a_part_finalized_tax_year_stops_the_next_run(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    finalize_march(&pool, &employer_id, &employment_id).await;

    // The forward claim the freeze check cannot disprove: it is April 2026,
    // but the caller names the next TaxYear and the change goes through.
    change_pay_schedule(
        &pool,
        &employer_id,
        twenty_fifth_schedule(),
        TaxYear::starting(2027),
        "actor",
    )
    .await
    .unwrap();

    // April 2026 under the moved schedule would make TaxYear 2026 hold a
    // period ending 31 March and one ending 25 April — thirteen periods, and
    // cumulative PAYE built on a boundary that no longer exists.
    let result = create_ordinary_payroll_run(
        &pool,
        &employer_id,
        PayPeriod::new(date(2026, 3, 26), date(2026, 4, 25)).unwrap(),
        date(2026, 5, 5),
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::PayScheduleMovedWithinTaxYear {
            employer_id: employer_id.clone(),
            tax_year: TaxYear::starting(2026),
            finalized_period_end: date(2026, 3, 31),
        })
    );
}

/// A TaxYear the moved schedule still describes is unaffected: the guard
/// names the finalized periods, not the change.
#[sqlx::test]
async fn a_run_in_the_next_tax_year_is_unaffected_by_the_change(pool: PgPool) {
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

    create_ordinary_payroll_run(
        &pool,
        &employer_id,
        PayPeriod::new(date(2027, 3, 26), date(2027, 4, 25)).unwrap(),
        date(2027, 5, 5),
        "actor",
    )
    .await
    .unwrap();
}

/// An unfinalized run in that TaxYear refuses the change: its period was cut
/// by the old schedule, and finalization re-derives from the current one.
#[sqlx::test]
async fn a_pay_schedule_change_is_refused_while_a_run_in_that_tax_year_is_open(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    create_ordinary_payroll_run(&pool, &employer_id, period(), date(2026, 4, 5), "actor")
        .await
        .unwrap();

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
        Err(PayrollAppError::PayScheduleChangeBlockedByAnOpenRun {
            employer_id: employer_id.clone(),
            period_end: period().end(),
        })
    );
}

/// An open run in a *different* TaxYear does not block the change.
#[sqlx::test]
async fn an_open_run_in_another_tax_year_does_not_block_the_change(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    create_ordinary_payroll_run(&pool, &employer_id, period(), date(2026, 4, 5), "actor")
        .await
        .unwrap();

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

// ---- A PaySchedule change must not strand a stored boundary (§4.2, §4.4, §4.5) ----

/// A `SaltCoverageStart` already on record must remain a `PayPeriod` end the
/// new schedule generates — the same demand §4.5 guard 1 makes when the row
/// is first written, now asked of a schedule change instead.
#[sqlx::test]
async fn a_pay_schedule_change_is_refused_when_it_would_strand_a_stored_salt_coverage_start(
    pool: PgPool,
) {
    let employer_id = an_employer(&pool).await;
    let employment_id = create_employment(
        &pool,
        &employer_id,
        &PersonId::new("person-1"),
        date(2026, 6, 1),
        None,
        "actor",
    )
    .await
    .unwrap();
    // June is this Employment's own first payable period end, so zero
    // figures over that empty span are the legitimate boundary here.
    record_opening_balance(
        &pool,
        &employment_id,
        TaxYear::starting(2026),
        date(2026, 6, 30),
        Money::ZERO,
        Money::ZERO,
        "actor",
    )
    .await
    .unwrap();

    // A day-25 schedule's periods end on the 25th, not the 30th: the
    // recorded boundary would be stranded mid-period.
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
        Err(
            PayrollAppError::PayScheduleChangeWouldStrandAStoredBoundary {
                employer_id: employer_id.clone(),
                fact: ScheduleBoundedFact::SaltCoverageStart,
                boundary: date(2026, 6, 30),
            }
        )
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

/// A `SaltCoverageStart` from a TaxYear earlier than the one the change
/// names describes a period already paid, so it is not checked — the same
/// exemption the finalization guard gives an earlier TaxYear (§4.2).
#[sqlx::test]
async fn a_stored_salt_coverage_start_in_an_earlier_tax_year_does_not_block_the_change(
    pool: PgPool,
) {
    let employer_id = an_employer(&pool).await;
    let employment_id = create_employment(
        &pool,
        &employer_id,
        &PersonId::new("person-1"),
        date(2025, 6, 1),
        None,
        "actor",
    )
    .await
    .unwrap();
    record_opening_balance(
        &pool,
        &employment_id,
        TaxYear::starting(2025),
        date(2025, 6, 30),
        Money::ZERO,
        Money::ZERO,
        "actor",
    )
    .await
    .unwrap();

    change_pay_schedule(
        &pool,
        &employer_id,
        twenty_fifth_schedule(),
        TaxYear::starting(2026),
        "actor",
    )
    .await
    .unwrap();
}

/// A void Employment's `OpeningBalance` cannot strand anything: a void
/// Employment reaches no payroll (§4.3), so no boundary of its own needs
/// protecting.
#[sqlx::test]
async fn a_void_employments_salt_coverage_start_does_not_block_the_change(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = create_employment(
        &pool,
        &employer_id,
        &PersonId::new("person-1"),
        date(2026, 6, 1),
        None,
        "actor",
    )
    .await
    .unwrap();
    record_opening_balance(
        &pool,
        &employment_id,
        TaxYear::starting(2026),
        date(2026, 6, 30),
        Money::ZERO,
        Money::ZERO,
        "actor",
    )
    .await
    .unwrap();
    void_employment(&pool, &employment_id, "actor")
        .await
        .unwrap();

    change_pay_schedule(
        &pool,
        &employer_id,
        twenty_fifth_schedule(),
        TaxYear::starting(2026),
        "actor",
    )
    .await
    .unwrap();
}

/// A `CompensationTerms.effective_from` already on record must remain a
/// `PayPeriod` start the new schedule generates (INV-014), just as it must
/// when the row is first written.
#[sqlx::test]
async fn a_pay_schedule_change_is_refused_when_it_would_strand_a_compensation_terms_effective_from(
    pool: PgPool,
) {
    let employer_id = an_employer(&pool).await;
    let employment_id = create_employment(
        &pool,
        &employer_id,
        &PersonId::new("person-1"),
        period().start(),
        None,
        "actor",
    )
    .await
    .unwrap();
    record_compensation_terms(
        &pool,
        &employment_id,
        period().start(),
        Money::from_cents(1_500_000).unwrap(),
        "actor",
    )
    .await
    .unwrap();

    // A day-25 schedule's periods start on the 26th: 1 March is no longer
    // the start of anything.
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
        Err(
            PayrollAppError::PayScheduleChangeWouldStrandAStoredBoundary {
                employer_id: employer_id.clone(),
                fact: ScheduleBoundedFact::CompensationTermsEffectiveFrom,
                boundary: period().start(),
            }
        )
    );
}

/// An `UnsupportedDeductionDeclaration.effective_from` already on record must
/// remain a `PayPeriod` start the new schedule generates (§4.5c), just as it
/// must when the row is first written.
#[sqlx::test]
async fn a_pay_schedule_change_is_refused_when_it_would_strand_an_unsupported_deduction_effective_from(
    pool: PgPool,
) {
    let employer_id = an_employer(&pool).await;
    let employment_id = create_employment(
        &pool,
        &employer_id,
        &PersonId::new("person-1"),
        period().start(),
        None,
        "actor",
    )
    .await
    .unwrap();
    declare_unsupported_deduction_status(
        &pool,
        &employment_id,
        period().start(),
        UnsupportedDeductionStatus::ConfirmedNone,
        "actor",
    )
    .await
    .unwrap();

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
        Err(
            PayrollAppError::PayScheduleChangeWouldStrandAStoredBoundary {
                employer_id: employer_id.clone(),
                fact: ScheduleBoundedFact::UnsupportedDeductionEffectiveFrom,
                boundary: period().start(),
            }
        )
    );
}

/// A schedule that still generates every stored boundary goes through
/// unrefused: the new guard only stops a change that would actually strand
/// something.
#[sqlx::test]
async fn a_pay_schedule_change_that_strands_nothing_is_allowed(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = create_employment(
        &pool,
        &employer_id,
        &PersonId::new("person-1"),
        period().start(),
        None,
        "actor",
    )
    .await
    .unwrap();
    record_compensation_terms(
        &pool,
        &employment_id,
        period().start(),
        Money::from_cents(1_500_000).unwrap(),
        "actor",
    )
    .await
    .unwrap();

    // Still a last-day-of-month schedule under the hood: every boundary
    // already recorded against the monthly schedule is unaffected.
    change_pay_schedule(
        &pool,
        &employer_id,
        monthly_schedule(),
        TaxYear::starting(2026),
        "actor",
    )
    .await
    .unwrap();
}

// ---- Concurrency: a freeze check and a finalization never interleave ----
//
// The freeze check is a read followed by a write, so on its own it is open to
// a finalization committing in between. What closes that is the row lock the
// two use cases take on the *same* `employment` row:
// `declare_prior_employment` and `record_opening_balance` take `FOR UPDATE`,
// and `finalize_payroll_run` takes `FOR SHARE` before it reads any master
// data. The two conflict, so one of them always waits for the other.
//
// Proved in the two steps `tests/finalize_payroll_run.rs` uses for the run
// lock: raw SQL on two real connections proves the lock genuinely blocks,
// then two real use cases in flight at once prove the outcome.
#[sqlx::test]
async fn a_frozen_fact_edit_and_a_finalization_never_both_succeed(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;

    // Phase 1: the `FOR SHARE` a finalization holds must block the
    // `FOR UPDATE` an edit of a frozen fact takes.
    let mut holder = pool
        .acquire()
        .await
        .expect("acquire the holding connection");
    let mut holder_tx = holder.begin().await.expect("begin the holding transaction");
    sqlx::query_scalar::<_, String>("SELECT id FROM employment WHERE id = $1 FOR SHARE")
        .bind(employment_id.as_str())
        .fetch_one(&mut *holder_tx)
        .await
        .expect("take the finalizer's row lock");

    let (started_sender, started_receiver) = oneshot::channel();
    let racing_pool = pool.clone();
    let racing_employment_id = employment_id.clone();
    let racing_task = tokio::spawn(async move {
        let mut conn = racing_pool
            .acquire()
            .await
            .expect("acquire racing connection");
        let mut tx = conn.begin().await.expect("begin racing transaction");
        started_sender.send(()).expect("notify the lock holder");
        sqlx::query_scalar::<_, String>("SELECT id FROM employment WHERE id = $1 FOR UPDATE")
            .bind(racing_employment_id.as_str())
            .fetch_one(&mut *tx)
            .await
            .expect("the row lock, once released, is granted here")
    });

    started_receiver.await.expect("racing transaction started");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !racing_task.is_finished(),
        "an edit of a frozen fact must wait for the lock a finalization holds"
    );
    holder_tx
        .rollback()
        .await
        .expect("release the lock without changing anything");
    racing_task.await.expect("join racing task");

    // Phase 2: a real edit and a real finalization, both in flight. Whichever
    // wins the row lock, the loser refuses — the edit because it now sees the
    // FinalizedPayroll, or the finalization because the fact it re-reads no
    // longer matches the one the run was calculated from (§5.1). Both
    // succeeding is the state ADR-0013 forbids: a frozen fact edited after
    // the finalization that re-reads it.
    let run_id =
        create_ordinary_payroll_run(&pool, &employer_id, period(), date(2026, 4, 5), "actor")
            .await
            .unwrap();
    let refusals = calculate_payroll_run(&pool, &run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(refusals, Vec::new(), "the run must reach Calculated");

    // The finalization is polled first, so it holds the row lock while the
    // edit tries to take it. That is the dangerous order: without the lock
    // the finalization would read the *committed* declaration — still the old
    // one, since the edit has not committed — pass its three-way comparison,
    // and then write a FinalizedPayroll on top of a fact that changed under
    // it, with both calls reporting success.
    let (finalization, edit) = tokio::join!(
        finalize_payroll_run(&pool, &run_id, "finalizer"),
        declare_prior_employment(
            &pool,
            &employment_id,
            TaxYear::starting(2026),
            PriorEmployment::Some(PriorEmploymentFigures::new(
                Money::from_cents(500_000).unwrap(),
                Money::from_cents(50_000).unwrap(),
            )),
            "actor",
        ),
    );

    assert!(
        edit.is_ok() != finalization.is_ok(),
        "exactly one must succeed: the loser of the row lock refuses, either \
         because it now sees the FinalizedPayroll or because the fact it \
         re-reads no longer matches the approved one — got {edit:?} and \
         {finalization:?}"
    );
}

/// The same conflict one level up: `change_pay_schedule` takes `FOR UPDATE`
/// on the `employer` row, and every use case that reads the schedule to
/// calculate against it takes `FOR SHARE` on the same row.
#[sqlx::test]
async fn a_pay_schedule_change_waits_for_a_reader_of_that_schedule(pool: PgPool) {
    let employer_id = an_employer(&pool).await;

    let mut holder = pool
        .acquire()
        .await
        .expect("acquire the holding connection");
    let mut holder_tx = holder.begin().await.expect("begin the holding transaction");
    sqlx::query_scalar::<_, String>(
        "SELECT period_end_day_kind FROM employer WHERE id = $1 FOR SHARE",
    )
    .bind(employer_id.as_str())
    .fetch_one(&mut *holder_tx)
    .await
    .expect("take the reader's row lock");

    let (started_sender, started_receiver) = oneshot::channel();
    let racing_pool = pool.clone();
    let racing_employer_id = employer_id.clone();
    let racing_task = tokio::spawn(async move {
        let mut conn = racing_pool
            .acquire()
            .await
            .expect("acquire racing connection");
        let mut tx = conn.begin().await.expect("begin racing transaction");
        started_sender.send(()).expect("notify the lock holder");
        sqlx::query_scalar::<_, String>(
            "SELECT period_end_day_kind FROM employer WHERE id = $1 FOR UPDATE",
        )
        .bind(racing_employer_id.as_str())
        .fetch_one(&mut *tx)
        .await
        .expect("the row lock, once released, is granted here")
    });

    started_receiver.await.expect("racing transaction started");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !racing_task.is_finished(),
        "a PaySchedule change must wait for a transaction already calculating \
         against that schedule"
    );
    holder_tx
        .rollback()
        .await
        .expect("release the lock without changing anything");
    racing_task.await.expect("join racing task");
}
