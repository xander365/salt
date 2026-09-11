//! Issue #78 (parent #70 §D-4) through the public use cases and a real
//! database: an employee's own medical aid premium is stored as a run pay
//! line beside earnings, withheld after tax, shown as its own figure, and
//! refused — never partially withheld — when it would take net pay below
//! zero. Employer-paid medical aid is refused by name.
//!
//! The figures whose values depend on Salt's no-relief reading are
//! `salt_policy_*` (SC-OPEN-7, `NEEDS NAMRA CONFIRMATION`), never
//! `statutory_*` (ADR-0008).

use chrono::NaiveDate;
use payroll::{
    DayOfMonth, EarningInstruction, EarningLabel, EmployerId, EmploymentId, Money, PayPeriod,
    PayrollError, PeriodEndDay, PriorEmployment, TaxYear, UnsupportedDeductionKind,
    UnsupportedDeductionKinds, UnsupportedDeductionStatus, VoluntaryDeductionInstruction,
};
use payroll_app::{
    CalculationState, EmploymentPerson, PayLineInstruction, PayLineSource, PayrollAppError,
    PayrollFigures, PayrollRunBlocker, PayrollRunDetail, PayrollRunId, RunStatus, SaltDatabase,
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

fn premium(cents: i64) -> VoluntaryDeductionInstruction {
    VoluntaryDeductionInstruction::MedicalAidPremium(Money::from_cents(cents).unwrap())
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

/// An Employment earning N$15,000.00 with every fact `calculate` needs on
/// record, and `unsupported` as its unsupported-deduction status.
async fn an_employment(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    person: &str,
    unsupported: UnsupportedDeductionStatus,
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
        unsupported,
        &[],
        "a reason",
        "actor",
    )
    .await
    .unwrap();
    employment_id
}

/// A run with two identical, calculable members, created but not
/// calculated.
async fn a_run_with_two_identical_members(
    db: &SaltDatabase,
) -> (EmployerId, PayrollRunId, EmploymentId, EmploymentId) {
    let employer_id = an_employer(db).await;
    let first = an_employment(
        db,
        &employer_id,
        "Ada Lovelace",
        UnsupportedDeductionStatus::ConfirmedNone,
    )
    .await;
    let second = an_employment(
        db,
        &employer_id,
        "Grace Hopper",
        UnsupportedDeductionStatus::ConfirmedNone,
    )
    .await;
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

fn member<'a>(
    detail: &'a PayrollRunDetail,
    employment_id: &EmploymentId,
) -> &'a payroll_app::PayrollRunMember {
    detail
        .members
        .iter()
        .find(|member| &member.employment_id == employment_id)
        .expect("the member is in the run")
}

fn figures(detail: &PayrollRunDetail, employment_id: &EmploymentId) -> PayrollFigures {
    member(detail, employment_id)
        .figures
        .expect("the member has current figures")
}

async fn pay_line_count(pool: &PgPool, run_id: &PayrollRunId, employment_id: &EmploymentId) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM payroll_run_pay_line
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(pool)
    .await
    .unwrap()
}

/// The first two acceptance criteria, through a real calculation: the member
/// with the premium differs from an identical member without it in net pay
/// alone, by exactly the premium, and every statutory figure is equal.
#[sqlx::test]
async fn salt_policy_a_stored_medical_aid_premium_lowers_only_net_pay_by_exactly_its_amount(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool);
    let (employer_id, run_id, with, without) = a_run_with_two_identical_members(&db).await;
    set_run_pay_lines(
        &db,
        &run_id,
        &with,
        vec![allowance(20_000)],
        vec![premium(75_000)],
    )
    .await
    .unwrap();
    set_run_pay_lines(&db, &run_id, &without, vec![allowance(20_000)], Vec::new())
        .await
        .unwrap();

    let refusals = calculate_payroll_run(&db, &run_id, "calculator")
        .await
        .unwrap();
    assert!(refusals.is_empty());

    let calculated = detail(&db, &employer_id, &run_id).await;
    let with = figures(&calculated, &with);
    let without = figures(&calculated, &without);

    assert_eq!(with.medical_aid_premium, Money::from_cents(75_000).unwrap());
    assert_eq!(without.medical_aid_premium, Money::ZERO);
    assert_eq!(
        with.net_pay,
        without
            .net_pay
            .checked_sub(Money::from_cents(75_000).unwrap())
            .unwrap()
    );
    assert_eq!(
        with.total_deductions,
        without
            .total_deductions
            .checked_add(Money::from_cents(75_000).unwrap())
            .unwrap()
    );
    // Bit-identical statutory arithmetic.
    assert_eq!(with.gross, without.gross);
    assert_eq!(with.taxable_remuneration, without.taxable_remuneration);
    assert_eq!(with.paye, without.paye);
    assert_eq!(
        with.employee_social_security,
        without.employee_social_security
    );
    assert_eq!(
        with.employer_social_security,
        without.employer_social_security
    );
}

/// Carried over from #77: a deduction is stored as a run pay line beside
/// the earnings, after them, each carrying its source.
#[sqlx::test]
async fn a_deduction_is_stored_as_a_run_pay_line_after_the_earnings_with_a_source(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (employer_id, run_id, first, _) = a_run_with_two_identical_members(&db).await;

    set_run_pay_lines(
        &db,
        &run_id,
        &first,
        vec![allowance(20_000)],
        vec![premium(75_000), premium(10_000)],
    )
    .await
    .unwrap();

    let stored = member(&detail(&db, &employer_id, &run_id).await, &first)
        .pay_lines
        .iter()
        .map(|line| (line.instruction.clone(), line.source))
        .collect::<Vec<_>>();
    assert_eq!(
        stored,
        vec![
            (
                PayLineInstruction::Earning(allowance(20_000)),
                PayLineSource::OneOff
            ),
            (
                PayLineInstruction::Deduction(premium(75_000)),
                PayLineSource::OneOff
            ),
            (
                PayLineInstruction::Deduction(premium(10_000)),
                PayLineSource::OneOff
            ),
        ]
    );
    assert_eq!(pay_line_count(&pool, &run_id, &first).await, 3);
}

/// The staleness rules #77 set for earnings hold for a deduction-only
/// change: it retires the member's figures and reopens the run, while
/// restating the same deduction changes nothing.
#[sqlx::test]
async fn a_deduction_only_change_retires_the_figures_and_an_unchanged_one_does_not(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let (employer_id, run_id, first, second) = a_run_with_two_identical_members(&db).await;
    set_run_pay_lines(&db, &run_id, &first, Vec::new(), vec![premium(75_000)])
        .await
        .unwrap();
    calculate_payroll_run(&db, &run_id, "calculator")
        .await
        .unwrap();

    set_run_pay_lines(&db, &run_id, &first, Vec::new(), vec![premium(75_000)])
        .await
        .unwrap();
    let unchanged = detail(&db, &employer_id, &run_id).await;
    assert_eq!(unchanged.status, RunStatus::Calculated);
    assert_eq!(
        member(&unchanged, &first).calculation_state,
        CalculationState::Current
    );

    set_run_pay_lines(&db, &run_id, &first, Vec::new(), vec![premium(80_000)])
        .await
        .unwrap();
    let changed = detail(&db, &employer_id, &run_id).await;
    assert_eq!(changed.status, RunStatus::Draft);
    assert_eq!(
        member(&changed, &first).calculation_state,
        CalculationState::PayLinesSaved
    );
    assert!(member(&changed, &first).figures.is_none());
    assert_eq!(
        member(&changed, &second).calculation_state,
        CalculationState::Current
    );
}

/// §D-4: `Money` refuses negative and fractional cents, and the write
/// boundary refuses zero — a zero deduction is not a line. Nothing is
/// written and true figures are not retired.
#[sqlx::test]
async fn a_zero_amount_deduction_is_refused_and_nothing_is_written(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (employer_id, run_id, first, _) = a_run_with_two_identical_members(&db).await;
    set_run_pay_lines(&db, &run_id, &first, vec![allowance(20_000)], Vec::new())
        .await
        .unwrap();
    calculate_payroll_run(&db, &run_id, "calculator")
        .await
        .unwrap();

    let refusal = set_run_pay_lines(
        &db,
        &run_id,
        &first,
        Vec::new(),
        vec![premium(75_000), premium(0)],
    )
    .await;

    assert_eq!(
        refusal,
        Err(PayrollAppError::VoluntaryDeductionAmountIsZero { index: 1 })
    );
    let after = detail(&db, &employer_id, &run_id).await;
    assert_eq!(after.status, RunStatus::Calculated);
    assert_eq!(
        member(&after, &first).calculation_state,
        CalculationState::Current
    );
    assert_eq!(
        member(&after, &first)
            .pay_lines
            .iter()
            .map(|line| line.instruction.clone())
            .collect::<Vec<_>>(),
        vec![PayLineInstruction::Earning(allowance(20_000))]
    );
}

/// Q-OPEN-22: a premium larger than the net pay left after PAYE and social
/// security is refused whole, naming the shortfall. Nothing is partially
/// withheld and no figures are stored for that member; the other member is
/// unaffected.
#[sqlx::test]
async fn salt_policy_a_premium_above_available_net_pay_is_refused_naming_the_exact_shortfall(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool);
    let (employer_id, run_id, first, second) = a_run_with_two_identical_members(&db).await;
    calculate_payroll_run(&db, &run_id, "calculator")
        .await
        .unwrap();
    let available = figures(&detail(&db, &employer_id, &run_id).await, &first).net_pay;

    set_run_pay_lines(
        &db,
        &run_id,
        &first,
        Vec::new(),
        vec![premium(available.cents() + 1_234)],
    )
    .await
    .unwrap();
    let refusals = calculate_payroll_run(&db, &run_id, "calculator")
        .await
        .unwrap();

    assert_eq!(refusals.len(), 1);
    assert_eq!(refusals[0].employment_id, first);
    assert_eq!(
        refusals[0].refusal,
        PayrollAppError::Payroll(PayrollError::DeductionsExceedGrossRemuneration {
            shortfall: Money::from_cents(1_234).unwrap()
        })
    );
    let after = detail(&db, &employer_id, &run_id).await;
    assert!(member(&after, &first).figures.is_none());
    assert!(member(&after, &second).figures.is_some());
}

/// Employer-paid medical aid joins the unsupported kinds: declarable, then
/// blocking by name through the existing present-kinds machinery, before any
/// pay line is set up.
#[sqlx::test]
async fn employer_paid_medical_aid_is_declarable_and_blocks_the_member_by_name(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    let kinds = UnsupportedDeductionKinds::new(vec![
        UnsupportedDeductionKind::EmployerPaidMedicalAidBenefit,
    ])
    .unwrap();
    let employment_id = an_employment(
        &db,
        &employer_id,
        "Ada Lovelace",
        UnsupportedDeductionStatus::Present(kinds.clone()),
    )
    .await;
    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();

    let before_calculating = detail(&db, &employer_id, &run_id).await;
    assert!(
        member(&before_calculating, &employment_id)
            .blockers
            .contains(&PayrollRunBlocker::UnsupportedDeductionsPresent {
                kinds: kinds.clone()
            })
    );

    let refusals = calculate_payroll_run(&db, &run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(
        refusals[0].refusal,
        PayrollAppError::Payroll(PayrollError::UnsupportedDeductionsPresent { kinds })
    );
}

/// Carried over from #77: a row written before issue #78 holds a bare
/// `EarningInstruction`. The run detail and Calculate both still read it —
/// no data migration was run, so the readers must.
#[sqlx::test]
async fn a_pre_deduction_bare_earning_row_still_reads_and_calculates(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (employer_id, run_id, first, _) = a_run_with_two_identical_members(&db).await;
    sqlx::query(
        "INSERT INTO payroll_run_pay_line
            (payroll_run_id, employment_id, line, pay_line_json, source)
         VALUES ($1::uuid, $2, 0, $3, 'one_off')",
    )
    .bind(run_id.as_str())
    .bind(first.as_str())
    .bind(serde_json::to_value(allowance(20_000)).unwrap())
    .execute(&pool)
    .await
    .unwrap();

    let before = detail(&db, &employer_id, &run_id).await;
    assert_eq!(
        member(&before, &first).pay_lines[0].instruction,
        PayLineInstruction::Earning(allowance(20_000))
    );

    calculate_payroll_run(&db, &run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(
        figures(&detail(&db, &employer_id, &run_id).await, &first).taxable_allowances,
        Money::from_cents(20_000).unwrap()
    );
}
