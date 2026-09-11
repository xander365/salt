//! Proves the defect issue #77 closes (parent #70 §D-6): writing, changing
//! or clearing a member's pay lines deletes that member's
//! `WorkingPayrollCalculation` in the same transaction, and the run detail
//! reports whether each member's figures are current or absent — and, when
//! absent, why. A figure on screen is never older than the inputs beside it.
//!
//! Driven through the public use cases with real calculations, not raw SQL,
//! so the states read back are the ones the worksheet would show.

use chrono::NaiveDate;
use payroll::{
    DayOfMonth, EarningInstruction, EarningLabel, EmployerId, EmploymentId, Money, PayPeriod,
    PeriodEndDay, PriorEmployment, TaxYear, UnsupportedDeductionStatus,
};
use payroll_app::{
    CalculationState, EmploymentPerson, PayrollRunDetail, PayrollRunId, RunStatus, SaltDatabase,
    calculate_payroll_run, create_employer, create_employment, create_ordinary_payroll_run,
    declare_prior_employment, declare_unsupported_deduction_status, get_payroll_run_detail,
    record_compensation_terms, set_run_pay_lines,
};
use sqlx::PgPool;

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// 2026-01-26 to 2026-02-25, one of the 25th-ending schedule's own periods.
fn period() -> PayPeriod {
    PayPeriod::new(date(2026, 1, 26), date(2026, 2, 25)).unwrap()
}

fn allowance(cents: i64) -> EarningInstruction {
    EarningInstruction::TaxableAllowance {
        amount: Money::from_cents(cents).unwrap(),
        label: Some(EarningLabel::new("standby allowance").unwrap()),
    }
}

async fn an_employer(db: &SaltDatabase) -> EmployerId {
    create_employer(
        db,
        "Employer",
        payroll::PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap())),
        "actor",
    )
    .await
    .unwrap()
}

/// An Employment with every fact `calculate` needs already on record.
async fn a_fully_declared_employment(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    person: &str,
) -> EmploymentId {
    let (_, employment_id) = create_employment(
        db,
        employer_id,
        EmploymentPerson::New(person.to_string()),
        date(2024, 1, 1),
        None,
        "actor",
    )
    .await
    .unwrap();
    record_compensation_terms(
        db,
        &employment_id,
        date(2025, 1, 26),
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
        date(2025, 1, 26),
        UnsupportedDeductionStatus::ConfirmedNone,
        &[],
        "a reason",
        "actor",
    )
    .await
    .unwrap();
    employment_id
}

/// A run with two calculable members, created but not calculated.
async fn a_run_with_two_members(
    db: &SaltDatabase,
) -> (EmployerId, PayrollRunId, EmploymentId, EmploymentId) {
    let employer_id = an_employer(db).await;
    let first = a_fully_declared_employment(db, &employer_id, "Ada Lovelace").await;
    let second = a_fully_declared_employment(db, &employer_id, "Grace Hopper").await;
    let run_id = create_ordinary_payroll_run(db, &employer_id, period(), date(2026, 3, 1), "actor")
        .await
        .unwrap();
    (employer_id, run_id, first, second)
}

async fn detail(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    run_id: &PayrollRunId,
) -> PayrollRunDetail {
    get_payroll_run_detail(db, employer_id, run_id.as_str())
        .await
        .unwrap()
}

/// `(calculation_state, figures present?)` for `employment_id` in `detail`.
fn state_of(detail: &PayrollRunDetail, employment_id: &EmploymentId) -> (CalculationState, bool) {
    let member = detail
        .members
        .iter()
        .find(|member| &member.employment_id == employment_id)
        .expect("the member is in the run");
    (member.calculation_state, member.figures.is_some())
}

async fn working_calculation_count(
    pool: &PgPool,
    run_id: &PayrollRunId,
    employment_id: &EmploymentId,
) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM working_payroll_calculation
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test]
async fn a_member_never_calculated_reports_not_calculated_even_after_a_write(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let (employer_id, run_id, first, _) = a_run_with_two_members(&db).await;

    assert_eq!(
        state_of(&detail(&db, &employer_id, &run_id).await, &first),
        (CalculationState::NotCalculated, false)
    );

    // No figures ever existed, so there is nothing a write could have made
    // stale — the worksheet must not say otherwise.
    set_run_pay_lines(&db, &run_id, &first, vec![allowance(50_000)], Vec::new())
        .await
        .unwrap();
    assert_eq!(
        state_of(&detail(&db, &employer_id, &run_id).await, &first),
        (CalculationState::NotCalculated, false)
    );
}

#[sqlx::test]
async fn writing_lines_retires_only_that_members_figures_until_the_next_calculate(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (employer_id, run_id, first, second) = a_run_with_two_members(&db).await;
    calculate_payroll_run(&db, &run_id, "calculator")
        .await
        .unwrap();
    let calculated = detail(&db, &employer_id, &run_id).await;
    assert_eq!(calculated.status, RunStatus::Calculated);
    assert_eq!(
        state_of(&calculated, &first),
        (CalculationState::Current, true)
    );

    set_run_pay_lines(&db, &run_id, &first, vec![allowance(50_000)], Vec::new())
        .await
        .unwrap();

    assert_eq!(working_calculation_count(&pool, &run_id, &first).await, 0);
    let after_write = detail(&db, &employer_id, &run_id).await;
    assert_eq!(after_write.status, RunStatus::Draft);
    assert_eq!(
        state_of(&after_write, &first),
        (CalculationState::PayLinesSaved, false)
    );
    // Another member's lines did not change, so their figures are still true.
    assert_eq!(
        state_of(&after_write, &second),
        (CalculationState::Current, true)
    );

    // A second write before any Calculate keeps saying why.
    set_run_pay_lines(&db, &run_id, &first, vec![allowance(60_000)], Vec::new())
        .await
        .unwrap();
    assert_eq!(
        state_of(&detail(&db, &employer_id, &run_id).await, &first),
        (CalculationState::PayLinesSaved, false)
    );

    calculate_payroll_run(&db, &run_id, "calculator")
        .await
        .unwrap();
    let recalculated = detail(&db, &employer_id, &run_id).await;
    assert_eq!(
        state_of(&recalculated, &first),
        (CalculationState::Current, true)
    );
    let first_member = recalculated
        .members
        .iter()
        .find(|member| member.employment_id == first)
        .unwrap();
    assert_eq!(
        first_member.figures.as_ref().unwrap().taxable_allowances,
        Money::from_cents(60_000).unwrap(),
        "the new figures are calculated from the lines now stored"
    );
}

/// Resubmitting exactly the stored lines changes no input, so it writes
/// nothing: the figures stay current and the run stays `Calculated`. Hiding
/// them would put a false "changed" on the worksheet (issue #88).
#[sqlx::test]
async fn resubmitting_the_same_lines_keeps_the_figures_current(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (employer_id, run_id, first, _) = a_run_with_two_members(&db).await;
    set_run_pay_lines(&db, &run_id, &first, vec![allowance(50_000)], Vec::new())
        .await
        .unwrap();
    calculate_payroll_run(&db, &run_id, "calculator")
        .await
        .unwrap();

    set_run_pay_lines(&db, &run_id, &first, vec![allowance(50_000)], Vec::new())
        .await
        .unwrap();

    assert_eq!(working_calculation_count(&pool, &run_id, &first).await, 1);
    let after = detail(&db, &employer_id, &run_id).await;
    assert_eq!(after.status, RunStatus::Calculated);
    assert_eq!(state_of(&after, &first), (CalculationState::Current, true));

    // The same instructions in a different order are a different statement.
    set_run_pay_lines(
        &db,
        &run_id,
        &first,
        vec![allowance(50_000), allowance(10_000)],
        Vec::new(),
    )
    .await
    .unwrap();
    calculate_payroll_run(&db, &run_id, "calculator")
        .await
        .unwrap();
    set_run_pay_lines(
        &db,
        &run_id,
        &first,
        vec![allowance(10_000), allowance(50_000)],
        Vec::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        state_of(&detail(&db, &employer_id, &run_id).await, &first),
        (CalculationState::PayLinesSaved, false)
    );
}

/// "Writing, changing or clearing": an empty list is a complete statement
/// (§4.5d), and it retires figures exactly as a longer list does.
#[sqlx::test]
async fn clearing_a_members_lines_retires_their_figures(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (employer_id, run_id, first, _) = a_run_with_two_members(&db).await;
    set_run_pay_lines(&db, &run_id, &first, vec![allowance(50_000)], Vec::new())
        .await
        .unwrap();
    calculate_payroll_run(&db, &run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(working_calculation_count(&pool, &run_id, &first).await, 1);

    set_run_pay_lines(&db, &run_id, &first, Vec::new(), Vec::new())
        .await
        .unwrap();

    assert_eq!(working_calculation_count(&pool, &run_id, &first).await, 0);
    assert_eq!(
        state_of(&detail(&db, &employer_id, &run_id).await, &first),
        (CalculationState::PayLinesSaved, false)
    );
}
