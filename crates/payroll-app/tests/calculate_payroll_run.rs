//! Proves the use cases issue #30 introduces: `calculate_payroll_run` and,
//! through it, `build_year_to_date_context` — `docs/domain/payroll-run-persistence.md`
//! §4.9 and §8 — reached through the public API a later ticket calls, not
//! raw SQL.

use chrono::NaiveDate;
use payroll::{
    DayOfMonth, Earning, EmployerId, EmploymentId, Money, PayPeriod, PayrollError, PeriodEndDay,
    PersonId, PriorEmployment, TaxYear, UnsupportedDeductionStatus,
};
use payroll_app::{
    PayrollAppError, PayrollRunCalculationRefusal, PayrollRunId, calculate_payroll_run,
    create_employer, create_employment, create_ordinary_payroll_run, declare_prior_employment,
    declare_unsupported_deduction_status, record_compensation_terms, remove_employment_from_run,
    set_run_earnings,
};
use sqlx::{PgPool, Row};

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn twenty_sixth_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap()))
}

/// 2026-01-26 to 2026-02-25 — one of `twenty_sixth_schedule()`'s own
/// periods, and the period every test in this file calculates.
fn period() -> PayPeriod {
    PayPeriod::new(date(2026, 1, 26), date(2026, 2, 25)).unwrap()
}

/// A date `twenty_sixth_schedule()` itself starts a period on, and well
/// before `period()` — used for every effective-dated declaration below,
/// so INV-014 is satisfied without the exact date mattering to any test.
fn well_before_the_period() -> NaiveDate {
    date(2025, 1, 26)
}

async fn an_employer(pool: &PgPool) -> EmployerId {
    create_employer(pool, twenty_sixth_schedule(), "actor")
        .await
        .unwrap()
}

/// An Employment with every fact `calculate` needs already on record:
/// CompensationTerms, a confirmed absence of PriorEmployment, and a
/// confirmed absence of unsupported deductions. Ready to calculate the
/// instant it is a run member.
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
        date(2024, 1, 1),
        None,
        "actor",
    )
    .await
    .unwrap();
    record_compensation_terms(
        pool,
        &employment_id,
        well_before_the_period(),
        basic_pay,
        "actor",
    )
    .await
    .unwrap();
    declare_prior_employment(
        pool,
        &employment_id,
        TaxYear::starting(2025),
        PriorEmployment::None,
        "actor",
    )
    .await
    .unwrap();
    declare_unsupported_deduction_status(
        pool,
        &employment_id,
        well_before_the_period(),
        UnsupportedDeductionStatus::ConfirmedNone,
        "actor",
    )
    .await
    .unwrap();
    employment_id
}

async fn run_status(pool: &PgPool, run_id: &PayrollRunId) -> String {
    sqlx::query_scalar("SELECT status FROM payroll_run WHERE id = $1::uuid")
        .bind(run_id.as_str())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn working_calculation_row(
    pool: &PgPool,
    run_id: &PayrollRunId,
    employment_id: &EmploymentId,
) -> Option<(
    serde_json::Value,
    serde_json::Value,
    serde_json::Value,
    String,
)> {
    sqlx::query(
        "SELECT payroll_input_json, payroll_rules_json, payroll_calculation_json, calculated_by
         FROM working_payroll_calculation
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_optional(pool)
    .await
    .unwrap()
    .map(|row| (row.get(0), row.get(1), row.get(2), row.get(3)))
}

// ---- CalculatePayrollRun (§4.9) ----

/// Every fact a member's `PayrollInput` needs is read on the transaction
/// `calculate_payroll_run` already opened, never on a second connection
/// borrowed from the pool. Two failures ride on that, and this test pins
/// the one that can be reproduced cheaply: a caller holding one pooled
/// connection while asking for another is how a pool of N deadlocks under N
/// concurrent callers. A pool of exactly one makes the single-caller case
/// of that hang immediately, so a regression here fails rather than waits
/// for load.
///
/// The other failure the same change prevents has no cheap test: a second
/// connection reads its own snapshot, so a `CompensationTerms` correction
/// committing mid-run could leave two members of one run calculated against
/// different facts.
#[sqlx::test]
async fn a_calculation_needs_only_the_one_connection_it_already_holds(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let run_id =
        create_ordinary_payroll_run(&pool, &employer_id, period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();
    set_run_earnings(&pool, &run_id, &employment_id, Vec::new())
        .await
        .unwrap();

    let single_connection_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect_with((*pool.connect_options()).clone())
        .await
        .unwrap();

    let refusals = calculate_payroll_run(&single_connection_pool, &run_id, "calculator")
        .await
        .expect("a calculation that needs a second connection times out here instead");

    assert_eq!(refusals, Vec::new());
    assert_eq!(run_status(&pool, &run_id).await, "calculated");
}

#[sqlx::test]
async fn a_fully_declared_single_member_run_calculates_and_becomes_calculated(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let run_id =
        create_ordinary_payroll_run(&pool, &employer_id, period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();

    let refusals = calculate_payroll_run(&pool, &run_id, "calculator")
        .await
        .unwrap();

    assert_eq!(refusals, Vec::new());
    assert_eq!(run_status(&pool, &run_id).await, "calculated");

    let (_, _, calculation_json, calculated_by) =
        working_calculation_row(&pool, &run_id, &employment_id)
            .await
            .expect("a WorkingPayrollCalculation row must exist");
    assert_eq!(calculated_by, "calculator");
    assert_eq!(
        calculation_json["gross_remuneration"],
        serde_json::json!(1500000)
    );
    assert_eq!(
        calculation_json["net_pay"],
        serde_json::json!(
            1500000
                - calculation_json["paye"]["amount"].as_i64().unwrap()
                - calculation_json["employee_social_security"]["amount"]
                    .as_i64()
                    .unwrap()
        )
    );
}

/// The whole loop an Employer actually walks: calculate, read the figures,
/// spot a wrong Earning, fix it, calculate again. The correction reopens the
/// run to `Draft` (§4.7) and the second calculation carries the new line.
#[sqlx::test]
async fn correcting_an_earning_reopens_a_calculated_run_and_the_next_calculation_carries_it(
    pool: PgPool,
) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let run_id =
        create_ordinary_payroll_run(&pool, &employer_id, period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();

    calculate_payroll_run(&pool, &run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(run_status(&pool, &run_id).await, "calculated");

    set_run_earnings(
        &pool,
        &run_id,
        &employment_id,
        vec![Earning::TaxableAllowance(
            Money::from_cents(250000).unwrap(),
        )],
    )
    .await
    .unwrap();
    assert_eq!(
        run_status(&pool, &run_id).await,
        "draft",
        "the stored calculation no longer accounts for the new line"
    );

    let refusals = calculate_payroll_run(&pool, &run_id, "calculator")
        .await
        .unwrap();

    assert_eq!(refusals, Vec::new());
    assert_eq!(run_status(&pool, &run_id).await, "calculated");
    let (input_json, _, calculation_json, _) =
        working_calculation_row(&pool, &run_id, &employment_id)
            .await
            .expect("the recalculated row");
    assert_eq!(
        input_json["earnings"].as_array().unwrap().len(),
        1,
        "the corrected Earning reached the calculation's input"
    );
    assert_eq!(
        calculation_json["gross_remuneration"],
        serde_json::json!(1500000 + 250000)
    );
}

#[sqlx::test]
async fn a_run_with_no_active_members_calculates_vacuously(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let run_id =
        create_ordinary_payroll_run(&pool, &employer_id, period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();

    let refusals = calculate_payroll_run(&pool, &run_id, "calculator")
        .await
        .unwrap();

    assert_eq!(refusals, Vec::new());
    assert_eq!(run_status(&pool, &run_id).await, "calculated");
}

#[sqlx::test]
async fn an_unknown_unsupported_deduction_status_blocks_only_that_member(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let ready = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-ready",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    // No `UnsupportedDeductionStatus` declared at all for this one.
    let blocked = create_employment(
        &pool,
        &employer_id,
        &PersonId::new("person-blocked"),
        date(2024, 1, 1),
        None,
        "actor",
    )
    .await
    .unwrap();
    record_compensation_terms(
        &pool,
        &blocked,
        well_before_the_period(),
        Money::from_cents(1200000).unwrap(),
        "actor",
    )
    .await
    .unwrap();
    declare_prior_employment(
        &pool,
        &blocked,
        TaxYear::starting(2025),
        PriorEmployment::None,
        "actor",
    )
    .await
    .unwrap();
    let run_id =
        create_ordinary_payroll_run(&pool, &employer_id, period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();

    let refusals = calculate_payroll_run(&pool, &run_id, "calculator")
        .await
        .unwrap();

    assert_eq!(
        refusals,
        vec![PayrollRunCalculationRefusal {
            employment_id: blocked.clone(),
            refusal: PayrollAppError::Payroll(PayrollError::UnsupportedDeductionStatusUnknown),
        }]
    );
    assert_eq!(
        run_status(&pool, &run_id).await,
        "draft",
        "a partially calculated run must stay Draft"
    );
    assert!(
        working_calculation_row(&pool, &run_id, &ready)
            .await
            .is_some()
    );
    assert!(
        working_calculation_row(&pool, &run_id, &blocked)
            .await
            .is_none()
    );
}

#[sqlx::test]
async fn an_unknown_prior_employment_blocks_only_that_member(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let blocked = create_employment(
        &pool,
        &employer_id,
        &PersonId::new("person-blocked"),
        date(2024, 1, 1),
        None,
        "actor",
    )
    .await
    .unwrap();
    record_compensation_terms(
        &pool,
        &blocked,
        well_before_the_period(),
        Money::from_cents(1200000).unwrap(),
        "actor",
    )
    .await
    .unwrap();
    declare_unsupported_deduction_status(
        &pool,
        &blocked,
        well_before_the_period(),
        UnsupportedDeductionStatus::ConfirmedNone,
        "actor",
    )
    .await
    .unwrap();
    // No `PriorEmployment` declared at all.
    let run_id =
        create_ordinary_payroll_run(&pool, &employer_id, period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();

    let refusals = calculate_payroll_run(&pool, &run_id, "calculator")
        .await
        .unwrap();

    assert_eq!(
        refusals,
        vec![PayrollRunCalculationRefusal {
            employment_id: blocked,
            refusal: PayrollAppError::Payroll(PayrollError::PriorEmploymentUnknown),
        }]
    );
    assert_eq!(run_status(&pool, &run_id).await, "draft");
}

#[sqlx::test]
async fn recalculating_overwrites_the_working_calculation_entirely(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    // A second, unresolved member keeps the run Draft across both
    // calculations, so `set_run_earnings` is still permitted between them.
    let unresolved = create_employment(
        &pool,
        &employer_id,
        &PersonId::new("person-unresolved"),
        date(2024, 1, 1),
        None,
        "actor",
    )
    .await
    .unwrap();
    let run_id =
        create_ordinary_payroll_run(&pool, &employer_id, period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();

    calculate_payroll_run(&pool, &run_id, "calculator")
        .await
        .unwrap();
    let (before_input, _, before_calc, _) = working_calculation_row(&pool, &run_id, &employment_id)
        .await
        .unwrap();
    assert_eq!(before_input["earnings"], serde_json::json!([]));

    set_run_earnings(
        &pool,
        &run_id,
        &employment_id,
        vec![Earning::TaxableAllowance(Money::from_cents(50000).unwrap())],
    )
    .await
    .unwrap();
    calculate_payroll_run(&pool, &run_id, "calculator")
        .await
        .unwrap();
    let (after_input, _, after_calc, _) = working_calculation_row(&pool, &run_id, &employment_id)
        .await
        .unwrap();

    assert_ne!(
        before_input, after_input,
        "the stored input must be the latest one"
    );
    assert_ne!(
        before_calc, after_calc,
        "the stored result must be the latest one, matching the latest input"
    );
    assert_eq!(
        after_calc["gross_remuneration"],
        serde_json::json!(1500000 + 50000)
    );

    // Only one row ever exists for this member: recalculation overwrites,
    // it does not accumulate a second row.
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM working_payroll_calculation
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let _ = unresolved;
}

#[sqlx::test]
async fn a_member_that_starts_failing_has_its_stale_working_calculation_cleared(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let run_id =
        create_ordinary_payroll_run(&pool, &employer_id, period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();

    calculate_payroll_run(&pool, &run_id, "calculator")
        .await
        .unwrap();
    assert!(
        working_calculation_row(&pool, &run_id, &employment_id)
            .await
            .is_some()
    );
    assert_eq!(run_status(&pool, &run_id).await, "calculated");

    // A new declaration taking effect exactly at this period's own start
    // supersedes the earlier `ConfirmedNone` for this calculation, without
    // touching run membership or anything the run-lock would gate.
    declare_unsupported_deduction_status(
        &pool,
        &employment_id,
        period().start(),
        UnsupportedDeductionStatus::Present(
            payroll::UnsupportedDeductionKinds::new(vec![
                payroll::UnsupportedDeductionKind::ProvidentFund,
            ])
            .unwrap(),
        ),
        "actor",
    )
    .await
    .unwrap();
    let refusals = calculate_payroll_run(&pool, &run_id, "calculator")
        .await
        .unwrap();

    assert_eq!(refusals.len(), 1);
    assert_eq!(refusals[0].employment_id, employment_id);
    assert!(matches!(
        refusals[0].refusal,
        PayrollAppError::Payroll(PayrollError::UnsupportedDeductionsPresent { .. })
    ));
    assert!(
        working_calculation_row(&pool, &run_id, &employment_id)
            .await
            .is_none(),
        "a stale WorkingPayrollCalculation must not survive a refusal"
    );
    assert_eq!(run_status(&pool, &run_id).await, "draft");
}

#[sqlx::test]
async fn a_removed_member_is_never_calculated(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let run_id =
        create_ordinary_payroll_run(&pool, &employer_id, period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();
    remove_employment_from_run(&pool, &run_id, &employment_id, "on unpaid leave", "actor")
        .await
        .unwrap();

    let refusals = calculate_payroll_run(&pool, &run_id, "calculator")
        .await
        .unwrap();

    assert_eq!(refusals, Vec::new());
    assert_eq!(run_status(&pool, &run_id).await, "calculated");
    assert!(
        working_calculation_row(&pool, &run_id, &employment_id)
            .await
            .is_none()
    );
}

/// A member removed *after* it calculated must not leave its
/// `WorkingPayrollCalculation` behind. The run reopens as `Draft` the moment
/// the member goes, but the next recalculation can make it `Calculated`
/// again from the members that remain — and a `Calculated` run holding a row
/// for someone the Employer deliberately, reasonedly took out of it is a
/// calculation that looks current and is not (§4.7, §4.9).
#[sqlx::test]
async fn removing_a_member_that_had_calculated_takes_its_working_calculation_with_it(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let stays = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let goes = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-2",
        Money::from_cents(900000).unwrap(),
    )
    .await;
    let run_id =
        create_ordinary_payroll_run(&pool, &employer_id, period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();
    calculate_payroll_run(&pool, &run_id, "calculator")
        .await
        .unwrap();
    assert!(
        working_calculation_row(&pool, &run_id, &goes)
            .await
            .is_some(),
        "the member must have calculated before it is removed"
    );

    remove_employment_from_run(
        &pool,
        &run_id,
        &goes,
        "resigned before the pay date",
        "actor",
    )
    .await
    .unwrap();

    assert!(
        working_calculation_row(&pool, &run_id, &goes)
            .await
            .is_none(),
        "a removed member keeps no working calculation"
    );

    let refusals = calculate_payroll_run(&pool, &run_id, "calculator")
        .await
        .unwrap();

    assert_eq!(refusals, Vec::new());
    assert_eq!(run_status(&pool, &run_id).await, "calculated");
    assert!(
        working_calculation_row(&pool, &run_id, &goes)
            .await
            .is_none(),
        "and a Calculated run never grows one back for it"
    );
    assert!(
        working_calculation_row(&pool, &run_id, &stays)
            .await
            .is_some()
    );
}

#[sqlx::test]
async fn recalculation_writes_no_action_log_entry(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let run_id =
        create_ordinary_payroll_run(&pool, &employer_id, period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();
    let before: i64 = sqlx::query_scalar("SELECT count(*) FROM action_log_entry")
        .fetch_one(&pool)
        .await
        .unwrap();

    calculate_payroll_run(&pool, &run_id, "calculator")
        .await
        .unwrap();
    calculate_payroll_run(&pool, &run_id, "calculator")
        .await
        .unwrap();

    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM action_log_entry")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before, after);
}

#[sqlx::test]
async fn a_missing_run_is_refused(pool: PgPool) {
    let made_up_id = create_ordinary_payroll_run(
        &pool,
        &an_employer(&pool).await,
        period(),
        date(2026, 3, 1),
        "actor",
    )
    .await
    .unwrap();
    sqlx::query("DELETE FROM payroll_run WHERE id = $1::uuid")
        .bind(made_up_id.as_str())
        .execute(&pool)
        .await
        .unwrap();

    let result = calculate_payroll_run(&pool, &made_up_id, "calculator").await;

    assert_eq!(result, Err(PayrollAppError::PayrollRunNotFound(made_up_id)));
}

#[sqlx::test]
async fn a_finalized_run_is_refused(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let run_id =
        create_ordinary_payroll_run(&pool, &employer_id, period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();
    // Nothing in this ticket can finalize a run — that is a separate, later
    // use case — so the state is written directly to exercise the refusal.
    sqlx::query("UPDATE payroll_run SET status = 'finalized' WHERE id = $1::uuid")
        .bind(run_id.as_str())
        .execute(&pool)
        .await
        .unwrap();

    let result = calculate_payroll_run(&pool, &run_id, "calculator").await;

    assert_eq!(
        result,
        Err(PayrollAppError::PayrollRunAlreadyFinalized(run_id))
    );
}
