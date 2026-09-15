//! Proves the use cases issue #80 introduces: `override_standing_pay_line`,
//! `remove_standing_pay_line`, and `refresh_standing_proposals`, plus the
//! change signal `get_payroll_run_detail` now carries and the provenance
//! `finalize_payroll_run` now freezes (`docs/domain/payroll-run-persistence.md`
//! §0, §D-6).

use chrono::NaiveDate;
use payroll::{
    DayOfMonth, EarningInstruction, EarningLabel, Money, PayPeriod, PeriodEndDay, PriorEmployment,
    TaxYear, UnsupportedDeductionStatus,
};
use payroll_app::{
    CalculationState, EmploymentPerson, PayLineInstruction, PayrollAppError, RunStatus,
    SaltDatabase, StandingPayItemInstruction, add_employment_to_correction_run,
    calculate_payroll_run, create_correction_run, create_employer, create_employment,
    create_ordinary_payroll_run, create_standing_pay_item, declare_prior_employment,
    declare_unsupported_deduction_status, end_standing_pay_item, finalize_payroll_run,
    get_finalized_payroll_detail, get_payroll_run_detail, override_standing_pay_line,
    record_compensation_terms, refresh_standing_proposals, remove_standing_pay_line,
    reverse_finalized_payroll, set_run_pay_lines,
};
use sqlx::PgPool;

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// A 26th-to-25th monthly schedule, matching `standing_pay_item.rs`.
fn twenty_sixth_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap()))
}

/// 2026-01-26 to 2026-02-25, one of `twenty_sixth_schedule()`'s own periods.
fn march_period() -> PayPeriod {
    PayPeriod::new(date(2026, 1, 26), date(2026, 2, 25)).unwrap()
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

/// An Employment with every fact `calculate` needs, starting on `start`, paid
/// N$15,000.00 from the first period start on or before it. Mirrors
/// `standing_pay_item.rs`'s own helper of the same name.
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
/// unchanged value proves a call wrote nothing at all — mirrors
/// `standing_pay_item.rs`'s own helper of the same name.
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

// ---- OverrideStandingPayLine ----

#[sqlx::test]
async fn an_override_changes_one_run_only_and_leaves_the_standing_item_untouched(pool: PgPool) {
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

    override_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_id.as_str(),
        standing_allowance(90_000),
        "temporary raise",
        "actor",
    )
    .await
    .unwrap();

    let items = payroll_app::list_standing_pay_items(&db, &employment_id)
        .await
        .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].instruction, standing_allowance(60_000));

    let next_period = PayPeriod::new(date(2026, 2, 26), date(2026, 3, 25)).unwrap();
    let next_run_id =
        create_ordinary_payroll_run(&db, &employer_id, next_period, date(2026, 4, 1), "actor")
            .await
            .unwrap();
    let detail = get_payroll_run_detail(&db, &employer_id, next_run_id.as_str())
        .await
        .unwrap();
    let line = &detail.members[0].pay_lines[0];
    assert_eq!(
        line.instruction,
        PayLineInstruction::Earning(allowance(60_000))
    );
    assert_eq!(line.override_reason, None);
}

#[sqlx::test]
async fn an_override_refuses_a_blank_reason(pool: PgPool) {
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

    let result = override_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_id.as_str(),
        standing_allowance(90_000),
        "   ",
        "actor",
    )
    .await;
    assert_eq!(result, Err(PayrollAppError::OverrideReasonCannotBeEmpty));
}

/// Leave payout, notice pay and severance are refused by name (issue #81,
/// D23), even as a one-run override — the operator cannot pay one under
/// cover of overriding an unrelated standing allowance's label.
#[sqlx::test]
async fn an_override_refuses_a_label_naming_an_out_of_scope_pay_type(pool: PgPool) {
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

    let result = override_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_id.as_str(),
        StandingPayItemInstruction::TaxableAllowance {
            amount: Money::from_cents(90_000).unwrap(),
            label: EarningLabel::new("notice pay").unwrap(),
        },
        "a reason",
        "actor",
    )
    .await;
    assert_eq!(
        result,
        Err(PayrollAppError::EarningLabelIsOutOfScope {
            label: "notice pay".to_string(),
        })
    );
}

#[sqlx::test]
async fn an_override_refuses_a_different_kind(pool: PgPool) {
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

    let result = override_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_id.as_str(),
        standing_medical_aid(15_000),
        "wrong kind",
        "actor",
    )
    .await;
    assert_eq!(
        result,
        Err(PayrollAppError::OverrideChangesPayLineKind {
            payroll_run_id: run_id.clone(),
            employment_id: employment_id.clone(),
            standing_pay_item_id: item_id.clone(),
        })
    );
}

/// Neither a one-off line nor a name that never belonged to a standing item
/// at all can be named by `standing_pay_item_id`: both read back as the same
/// refusal (ADR-0017's own "unknown and out of scope are indistinguishable").
#[sqlx::test]
async fn an_override_of_a_one_off_or_unknown_line_is_not_found(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;
    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();
    set_run_pay_lines(
        &db,
        &run_id,
        &employment_id,
        vec![allowance(10_000)],
        Vec::new(),
    )
    .await
    .unwrap();

    let unknown_id = uuid::Uuid::new_v4().to_string();
    let result = override_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        &unknown_id,
        standing_allowance(20_000),
        "a reason",
        "actor",
    )
    .await;
    assert!(matches!(
        result,
        Err(PayrollAppError::StandingPayLineNotFound { .. })
    ));
}

#[sqlx::test]
async fn overriding_back_to_the_standing_amount_clears_the_override(pool: PgPool) {
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

    override_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_id.as_str(),
        standing_allowance(90_000),
        "temporary raise",
        "actor",
    )
    .await
    .unwrap();
    override_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_id.as_str(),
        standing_allowance(60_000),
        "back to normal",
        "actor",
    )
    .await
    .unwrap();

    let detail = get_payroll_run_detail(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    let line = &detail.members[0].pay_lines[0];
    assert_eq!(
        line.instruction,
        PayLineInstruction::Earning(allowance(60_000))
    );
    assert_eq!(line.override_reason, None);
}

// ---- RemoveStandingPayLine ----

#[sqlx::test]
async fn a_removed_line_contributes_nothing_and_stays_visible_with_its_reason(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = a_payable_employment(
        &db,
        &employer_id,
        march_period().start(),
        march_period().start(),
    )
    .await;
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

    remove_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_id.as_str(),
        "not paid this month",
        "actor",
    )
    .await
    .unwrap();

    calculate_payroll_run(&db, &run_id, "calculator")
        .await
        .unwrap();
    let detail = get_payroll_run_detail(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    let member = &detail.members[0];
    let figures = member.figures.expect("this member calculated cleanly");
    assert_eq!(
        figures.taxable_allowances,
        Money::ZERO,
        "a removed allowance contributes nothing to the figures"
    );

    let removed_line = member
        .pay_lines
        .iter()
        .find(|line| line.is_removed())
        .expect("the removed line still appears in the detail");
    assert_eq!(
        removed_line.removed_reason,
        Some("not paid this month".to_string())
    );
}

#[sqlx::test]
async fn removing_twice_is_refused(pool: PgPool) {
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

    remove_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_id.as_str(),
        "first",
        "actor",
    )
    .await
    .unwrap();

    let result = remove_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_id.as_str(),
        "second",
        "actor",
    )
    .await;
    assert_eq!(
        result,
        Err(PayrollAppError::StandingPayLineAlreadyRemoved {
            payroll_run_id: run_id.clone(),
            employment_id: employment_id.clone(),
            standing_pay_item_id: item_id.clone(),
        })
    );
}

#[sqlx::test]
async fn overriding_a_removed_line_is_refused(pool: PgPool) {
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

    remove_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_id.as_str(),
        "not paid",
        "actor",
    )
    .await
    .unwrap();

    let result = override_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_id.as_str(),
        standing_allowance(90_000),
        "raise",
        "actor",
    )
    .await;
    assert_eq!(
        result,
        Err(PayrollAppError::StandingPayLineIsRemoved {
            payroll_run_id: run_id.clone(),
            employment_id: employment_id.clone(),
            standing_pay_item_id: item_id.clone(),
        })
    );
}

#[sqlx::test]
async fn a_removal_refuses_a_blank_reason(pool: PgPool) {
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

    let result = remove_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_id.as_str(),
        "  ",
        "actor",
    )
    .await;
    assert_eq!(
        result,
        Err(PayrollAppError::PayLineRemovalReasonCannotBeEmpty)
    );
}

// ---- Overrides and removals under calculation and lifecycle ----

#[sqlx::test]
async fn an_override_and_a_removal_survive_recalculation(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = a_payable_employment(
        &db,
        &employer_id,
        march_period().start(),
        march_period().start(),
    )
    .await;
    let item_a = create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(60_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();
    let item_b = create_standing_pay_item(
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

    override_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_a.as_str(),
        standing_allowance(90_000),
        "raise",
        "actor",
    )
    .await
    .unwrap();
    remove_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_b.as_str(),
        "not paid",
        "actor",
    )
    .await
    .unwrap();

    calculate_payroll_run(&db, &run_id, "actor").await.unwrap();
    // Recalculation.
    calculate_payroll_run(&db, &run_id, "actor").await.unwrap();

    let detail = get_payroll_run_detail(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    let lines = &detail.members[0].pay_lines;
    let line_a = lines
        .iter()
        .find(|line| line.standing_pay_item_id.as_ref() == Some(&item_a))
        .unwrap();
    assert_eq!(line_a.override_reason, Some("raise".to_string()));
    let line_b = lines
        .iter()
        .find(|line| line.standing_pay_item_id.as_ref() == Some(&item_b))
        .unwrap();
    assert!(line_b.is_removed());
}

#[sqlx::test]
async fn an_override_retires_the_members_figures_and_reopens_the_run(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = a_payable_employment(
        &db,
        &employer_id,
        march_period().start(),
        march_period().start(),
    )
    .await;
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
    calculate_payroll_run(&db, &run_id, "actor").await.unwrap();

    override_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_id.as_str(),
        standing_allowance(90_000),
        "raise",
        "actor",
    )
    .await
    .unwrap();

    let detail = get_payroll_run_detail(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    assert_eq!(detail.status, RunStatus::Draft);
    assert_eq!(
        detail.members[0].calculation_state,
        CalculationState::PayLinesSaved
    );
}

#[sqlx::test]
async fn override_remove_and_refresh_refuse_a_finalized_run(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = a_payable_employment(
        &db,
        &employer_id,
        march_period().start(),
        march_period().start(),
    )
    .await;
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
    calculate_payroll_run(&db, &run_id, "actor").await.unwrap();
    finalize_payroll_run(&db, &run_id, "actor").await.unwrap();

    let override_result = override_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_id.as_str(),
        standing_allowance(90_000),
        "raise",
        "actor",
    )
    .await;
    assert!(matches!(
        override_result,
        Err(PayrollAppError::PayrollRunAlreadyFinalized { .. })
    ));

    let remove_result = remove_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_id.as_str(),
        "reason",
        "actor",
    )
    .await;
    assert!(matches!(
        remove_result,
        Err(PayrollAppError::PayrollRunAlreadyFinalized { .. })
    ));

    let refresh_result = refresh_standing_proposals(&db, &run_id, "actor").await;
    assert!(matches!(
        refresh_result,
        Err(PayrollAppError::PayrollRunAlreadyFinalized { .. })
    ));
}

#[sqlx::test]
async fn refresh_refuses_a_correction_run(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let run_id = create_correction_run(
        &db,
        &employer_id,
        march_period(),
        date(2026, 3, 1),
        "fixing march",
        "actor",
    )
    .await
    .unwrap();

    let result = refresh_standing_proposals(&db, &run_id, "actor").await;
    assert_eq!(
        result,
        Err(PayrollAppError::PayrollRunIsNotOrdinary(run_id.clone()))
    );
}

#[sqlx::test]
async fn a_resave_keeps_override_reasons_and_removed_lines_and_writes_nothing(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;
    let item_a = create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(60_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();
    let item_b = create_standing_pay_item(
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

    override_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_a.as_str(),
        standing_allowance(90_000),
        "raise",
        "actor",
    )
    .await
    .unwrap();
    remove_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_b.as_str(),
        "not paid",
        "actor",
    )
    .await
    .unwrap();

    let before = stored_lines(&pool, &run_id).await;

    // Restates the overridden line exactly (its current, overridden
    // instruction) and omits the removed line, per §0's decision 3.
    set_run_pay_lines(
        &db,
        &run_id,
        &employment_id,
        vec![allowance(90_000)],
        Vec::new(),
    )
    .await
    .unwrap();

    let after = stored_lines(&pool, &run_id).await;
    assert_eq!(
        before, after,
        "a resave that changes nothing must write nothing"
    );
}

// ---- The change signal ----

#[sqlx::test]
async fn the_detail_reports_items_added_and_ended_since_proposal(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;
    let item_a = create_standing_pay_item(
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

    end_standing_pay_item(
        &db,
        &employment_id,
        item_a.as_str(),
        "no longer applicable",
        "actor",
    )
    .await
    .unwrap();
    let item_b = create_standing_pay_item(
        &db,
        &employment_id,
        standing_medical_aid(15_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();

    let detail = get_payroll_run_detail(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    let changed = &detail.members[0].standing_items_changed;
    assert_eq!(changed.added.len(), 1);
    assert_eq!(changed.added[0].standing_pay_item_id, item_b);
    assert_eq!(changed.ended.len(), 1);
    assert_eq!(changed.ended[0].standing_pay_item_id, item_a);
}

#[sqlx::test]
async fn a_correction_run_reports_no_standing_changes(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = a_payable_employment(
        &db,
        &employer_id,
        march_period().start(),
        march_period().start(),
    )
    .await;
    create_standing_pay_item(
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
    reverse_finalized_payroll(&db, &target, "wrong", "actor")
        .await
        .unwrap();

    let correction_run_id = create_correction_run(
        &db,
        &employer_id,
        march_period(),
        date(2026, 4, 1),
        "fixing march",
        "actor",
    )
    .await
    .unwrap();
    add_employment_to_correction_run(
        &db,
        &correction_run_id,
        &employment_id,
        Some(&target),
        "actor",
    )
    .await
    .unwrap();

    let detail = get_payroll_run_detail(&db, &employer_id, correction_run_id.as_str())
        .await
        .unwrap();
    assert!(detail.members[0].standing_items_changed.is_empty());
}

// ---- RefreshStandingProposals ----

#[sqlx::test]
async fn refresh_adds_a_newly_in_force_item_and_leaves_overridden_and_removed_lines_untouched(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;
    let item_a = create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(60_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();
    let item_b = create_standing_pay_item(
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

    override_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_a.as_str(),
        standing_allowance(90_000),
        "raise",
        "actor",
    )
    .await
    .unwrap();
    remove_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_b.as_str(),
        "not paid",
        "actor",
    )
    .await
    .unwrap();

    let item_c = create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(20_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();

    let report = refresh_standing_proposals(&db, &run_id, "actor")
        .await
        .unwrap();
    assert_eq!(report.members.len(), 1);
    let member = &report.members[0];
    assert_eq!(member.added.len(), 1);
    assert_eq!(member.added[0].standing_pay_item_id, item_c);
    assert_eq!(member.kept_overridden, vec![item_a.clone()]);
    assert_eq!(member.kept_removed, vec![item_b.clone()]);

    let detail = get_payroll_run_detail(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    let lines = &detail.members[0].pay_lines;
    assert!(lines.iter().any(|line| {
        line.standing_pay_item_id.as_ref() == Some(&item_a) && line.override_reason.is_some()
    }));
    assert!(
        lines
            .iter()
            .any(|line| line.standing_pay_item_id.as_ref() == Some(&item_b) && line.is_removed())
    );
    assert!(
        lines
            .iter()
            .any(|line| line.standing_pay_item_id.as_ref() == Some(&item_c))
    );
}

#[sqlx::test]
async fn refresh_reports_an_ended_item_and_does_not_delete_its_line(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;
    let item_a = create_standing_pay_item(
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

    end_standing_pay_item(
        &db,
        &employment_id,
        item_a.as_str(),
        "no longer needed",
        "actor",
    )
    .await
    .unwrap();

    let before = stored_lines(&pool, &run_id).await;
    let report = refresh_standing_proposals(&db, &run_id, "actor")
        .await
        .unwrap();
    let after = stored_lines(&pool, &run_id).await;
    assert_eq!(
        before, after,
        "an ended item's line must not be deleted or otherwise rewritten"
    );

    assert_eq!(report.members.len(), 1);
    assert_eq!(report.members[0].ended_still_proposed.len(), 1);
    assert_eq!(
        report.members[0].ended_still_proposed[0].standing_pay_item_id,
        item_a
    );
}

#[sqlx::test]
async fn refreshing_twice_with_nothing_changed_writes_nothing(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = a_payable_employment(
        &db,
        &employer_id,
        march_period().start(),
        march_period().start(),
    )
    .await;
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
    calculate_payroll_run(&db, &run_id, "actor").await.unwrap();

    let report1 = refresh_standing_proposals(&db, &run_id, "actor")
        .await
        .unwrap();
    assert!(report1.members.is_empty());

    let detail_after_first = get_payroll_run_detail(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    assert_eq!(detail_after_first.status, RunStatus::Calculated);

    let before = stored_lines(&pool, &run_id).await;
    let log_count_before: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM action_log_entry WHERE action_type = 'standing_proposals_refreshed'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        log_count_before, 0,
        "nothing changed, so no entry was written yet"
    );

    let report2 = refresh_standing_proposals(&db, &run_id, "actor")
        .await
        .unwrap();
    assert!(report2.members.is_empty());
    let after = stored_lines(&pool, &run_id).await;
    assert_eq!(before, after);

    let log_count_after: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM action_log_entry WHERE action_type = 'standing_proposals_refreshed'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        log_count_after, 0,
        "still no entry: neither refresh wrote anything"
    );

    let detail_after_second = get_payroll_run_detail(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    assert_eq!(
        detail_after_second.status,
        RunStatus::Calculated,
        "status stays Calculated since nothing was written"
    );
}

#[sqlx::test]
async fn two_concurrent_refreshes_add_each_item_once(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = an_employment(&db, &employer_id).await;
    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();
    let item_id = create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(60_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();

    let (result_a, result_b) = tokio::join!(
        refresh_standing_proposals(&db, &run_id, "actor-a"),
        refresh_standing_proposals(&db, &run_id, "actor-b"),
    );
    result_a.unwrap();
    result_b.unwrap();

    let rows: Vec<(Option<String>,)> = sqlx::query_as(
        "SELECT standing_pay_item_id::text FROM payroll_run_pay_line
         WHERE payroll_run_id = $1::uuid AND standing_pay_item_id = $2::uuid",
    )
    .bind(run_id.as_str())
    .bind(item_id.as_str())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows.len(),
        1,
        "the item must be added exactly once, not twice"
    );
}

// ---- Frozen provenance ----

#[sqlx::test]
async fn finalization_freezes_line_provenance(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = a_payable_employment(
        &db,
        &employer_id,
        march_period().start(),
        march_period().start(),
    )
    .await;
    let item_a = create_standing_pay_item(
        &db,
        &employment_id,
        standing_allowance(60_000),
        march_period().start(),
        "actor",
    )
    .await
    .unwrap();
    let item_b = create_standing_pay_item(
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

    override_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_a.as_str(),
        standing_allowance(90_000),
        "raise",
        "actor",
    )
    .await
    .unwrap();
    remove_standing_pay_line(
        &db,
        &run_id,
        &employment_id,
        item_b.as_str(),
        "not paid",
        "actor",
    )
    .await
    .unwrap();

    calculate_payroll_run(&db, &run_id, "actor").await.unwrap();
    let outcome = finalize_payroll_run(&db, &run_id, "actor").await.unwrap();
    let finalized_id = outcome.finalized[0].1.clone();

    let detail = get_finalized_payroll_detail(&db, &employer_id, finalized_id.as_str())
        .await
        .unwrap();
    let lines = detail
        .pay_line_provenance
        .expect("issue #80 always freezes provenance");
    let line_a = lines
        .iter()
        .find(|line| line.standing_pay_item_id.as_deref() == Some(item_a.as_str()))
        .unwrap();
    assert_eq!(line_a.override_reason, Some("raise".to_string()));
    let line_b = lines
        .iter()
        .find(|line| line.standing_pay_item_id.as_deref() == Some(item_b.as_str()))
        .unwrap();
    assert_eq!(line_b.removed_reason, Some("not paid".to_string()));

    let schema_version: i32 = sqlx::query_scalar(
        "SELECT snapshot_schema_version FROM finalized_payroll WHERE id = $1::uuid",
    )
    .bind(finalized_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(schema_version, 5);
}
