//! Proves the use case issue #35 introduces: `CreateCorrectionRun` and
//! `AddEmploymentToCorrectionRun` — `docs/domain/payroll-run-persistence.md`
//! §4.8, §6.3, §6.5, §7.4, ADR-0015 — and `FinalizePayrollRun`'s own
//! Correction column of §5.3 step 3, reached through the public API.

use chrono::NaiveDate;
use payroll::{
    Earning, EmployerId, EmploymentId, Money, PayPeriod, PeriodEndDay, PersonId, PriorEmployment,
    TaxYear, UnsupportedDeductionStatus,
};
use payroll_app::{
    EarningPrePopulation, FinalizedPayrollId, PayrollAppError, PayrollRunId,
    SNAPSHOT_SCHEMA_VERSION, add_employment_to_correction_run, calculate_payroll_run,
    correct_compensation_terms, create_correction_run, create_employer, create_employment,
    create_ordinary_payroll_run, declare_prior_employment, declare_unsupported_deduction_status,
    finalize_payroll_run, record_compensation_terms, remove_employment_from_run,
    reverse_finalized_payroll, set_run_earnings,
};
use sqlx::PgPool;

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn monthly_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::LastDayOfMonth)
}

fn period() -> PayPeriod {
    PayPeriod::new(date(2026, 3, 1), date(2026, 3, 31)).unwrap()
}

fn next_period() -> PayPeriod {
    PayPeriod::new(date(2026, 4, 1), date(2026, 4, 30)).unwrap()
}

async fn an_employer(pool: &PgPool) -> EmployerId {
    create_employer(pool, monthly_schedule(), "actor")
        .await
        .unwrap()
}

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
    record_compensation_terms(
        pool,
        &employment_id,
        period().start(),
        basic_pay,
        &[],
        "",
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
        &[],
        "a reason",
        "actor",
    )
    .await
    .unwrap();
    employment_id
}

/// Creates and finalizes an Ordinary March run for `employment_id`, and
/// returns its `FinalizedPayrollId`. Live: nothing reverses it.
async fn finalize_march(
    pool: &PgPool,
    employer_id: &EmployerId,
    employment_id: &EmploymentId,
) -> FinalizedPayrollId {
    let run_id =
        create_ordinary_payroll_run(pool, employer_id, period(), date(2026, 4, 5), "actor")
            .await
            .unwrap();
    calculate_payroll_run(pool, &run_id, "calculator")
        .await
        .unwrap();
    let outcome = finalize_payroll_run(pool, &run_id, "finalizer")
        .await
        .unwrap();
    outcome
        .finalized
        .into_iter()
        .find(|(id, _)| id == employment_id)
        .expect("the Employment must have finalized")
        .1
}

/// As [`finalize_march`], but reverses the result immediately afterwards —
/// the fixture every replacement test starts from (§6.1, §6.3).
async fn finalize_and_reverse_march(
    pool: &PgPool,
    employer_id: &EmployerId,
    employment_id: &EmploymentId,
) -> FinalizedPayrollId {
    let finalized_payroll_id = finalize_march(pool, employer_id, employment_id).await;
    reverse_finalized_payroll(
        pool,
        &finalized_payroll_id,
        "March salary was wrong",
        "actor",
    )
    .await
    .unwrap();
    finalized_payroll_id
}

/// Creates, calculates and finalizes a Correction run for `period()` holding
/// `employment_id` alone, declaring `replaces`, and returns the
/// `FinalizedPayrollId` it minted — one link of the chain.
async fn finalize_a_correction(
    pool: &PgPool,
    employer_id: &EmployerId,
    employment_id: &EmploymentId,
    replaces: Option<&FinalizedPayrollId>,
) -> FinalizedPayrollId {
    let run_id = create_correction_run(
        pool,
        employer_id,
        period(),
        date(2026, 7, 5),
        "the figure was wrong",
        "actor",
    )
    .await
    .unwrap();
    add_employment_to_correction_run(pool, &run_id, employment_id, replaces, "actor")
        .await
        .unwrap();
    calculate_payroll_run(pool, &run_id, "calculator")
        .await
        .unwrap();
    let outcome = finalize_payroll_run(pool, &run_id, "finalizer")
        .await
        .unwrap();
    outcome
        .finalized
        .into_iter()
        .find(|(id, _)| id == employment_id)
        .expect("the Correction's one member must have finalized")
        .1
}

async fn run_status(pool: &PgPool, run_id: &PayrollRunId) -> String {
    sqlx::query_scalar("SELECT status FROM payroll_run WHERE id = $1::uuid")
        .bind(run_id.as_str())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn member_count(pool: &PgPool, run_id: &PayrollRunId) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM payroll_run_employment WHERE payroll_run_id = $1::uuid",
    )
    .bind(run_id.as_str())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn live_finalized_payroll_id(
    pool: &PgPool,
    employment_id: &EmploymentId,
    period_end: NaiveDate,
) -> Option<String> {
    sqlx::query_scalar(
        "SELECT finalized_payroll_id::text FROM live_finalized_payroll
         WHERE employment_id = $1 AND period_end = $2",
    )
    .bind(employment_id.as_str())
    .bind(period_end)
    .fetch_optional(pool)
    .await
    .unwrap()
}

async fn finalized_payroll_count(pool: &PgPool, employment_id: &EmploymentId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM finalized_payroll WHERE employment_id = $1")
        .bind(employment_id.as_str())
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The whole `finalized_payroll` row, as JSON, so a test can say "untouched"
/// about every column rather than about the two it thought to name.
async fn finalized_payroll_row(
    pool: &PgPool,
    finalized_payroll_id: &FinalizedPayrollId,
) -> serde_json::Value {
    sqlx::query_scalar("SELECT to_jsonb(f) FROM finalized_payroll AS f WHERE f.id = $1::uuid")
        .bind(finalized_payroll_id.as_str())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn action_log_context(
    pool: &PgPool,
    action_type: &str,
    target_id: &str,
) -> serde_json::Value {
    sqlx::query_scalar(
        "SELECT context FROM action_log_entry WHERE action_type = $1 AND target_id = $2",
    )
    .bind(action_type)
    .bind(target_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

// ---- CreateCorrectionRun (§4.6, §4.8) ----

#[sqlx::test]
async fn create_correction_run_creates_a_draft_run_that_proposes_nobody(pool: PgPool) {
    let employer_id = an_employer(&pool).await;

    let run_id = create_correction_run(
        &pool,
        &employer_id,
        period(),
        date(2026, 6, 5),
        "March pay was wrong",
        "actor",
    )
    .await
    .unwrap();

    assert_eq!(run_status(&pool, &run_id).await, "draft");
    assert_eq!(member_count(&pool, &run_id).await, 0);
}

#[sqlx::test]
async fn create_correction_run_refuses_a_blank_reason(pool: PgPool) {
    let employer_id = an_employer(&pool).await;

    let result = create_correction_run(
        &pool,
        &employer_id,
        period(),
        date(2026, 6, 5),
        "   ",
        "actor",
    )
    .await;

    assert_eq!(result, Err(PayrollAppError::CorrectionReasonCannotBeEmpty));
}

// ---- Membership: exactly one Employment (§4.8, ADR-0015) ----

#[sqlx::test]
async fn a_second_employment_is_refused(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_a = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-a",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let employment_b = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-b",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let run_id = create_correction_run(
        &pool,
        &employer_id,
        period(),
        date(2026, 6, 5),
        "March pay was wrong",
        "actor",
    )
    .await
    .unwrap();
    add_employment_to_correction_run(&pool, &run_id, &employment_a, None, "actor")
        .await
        .unwrap();

    let result =
        add_employment_to_correction_run(&pool, &run_id, &employment_b, None, "actor").await;

    assert_eq!(
        result,
        Err(PayrollAppError::CorrectionRunAlreadyHasAnEmployment(run_id))
    );
}

// ---- The declared target (§4.8) ----

#[sqlx::test]
async fn a_target_with_no_reversal_is_refused(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let finalized_payroll_id = finalize_march(&pool, &employer_id, &employment_id).await;
    let run_id = create_correction_run(
        &pool,
        &employer_id,
        period(),
        date(2026, 6, 5),
        "March pay was wrong",
        "actor",
    )
    .await
    .unwrap();

    let result = add_employment_to_correction_run(
        &pool,
        &run_id,
        &employment_id,
        Some(&finalized_payroll_id),
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::CorrectionTargetNotReversed(
            finalized_payroll_id
        ))
    );
}

#[sqlx::test]
async fn a_target_belonging_to_a_different_employment_is_refused(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_a = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-a",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let employment_b = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-b",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let target_a = finalize_and_reverse_march(&pool, &employer_id, &employment_a).await;
    let run_id = create_correction_run(
        &pool,
        &employer_id,
        period(),
        date(2026, 6, 5),
        "March pay was wrong",
        "actor",
    )
    .await
    .unwrap();

    let result =
        add_employment_to_correction_run(&pool, &run_id, &employment_b, Some(&target_a), "actor")
            .await;

    assert_eq!(
        result,
        Err(PayrollAppError::CorrectionTargetDoesNotMatch {
            payroll_run_id: run_id,
            finalized_payroll_id: target_a,
        })
    );
}

// ---- A reversed record is replaceable at most once (§4.8, §9) ----

#[sqlx::test]
async fn two_draft_corrections_naming_the_same_target_end_with_only_the_first_finalized(
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
    let target = finalize_and_reverse_march(&pool, &employer_id, &employment_id).await;

    let run_a = create_correction_run(
        &pool,
        &employer_id,
        period(),
        date(2026, 6, 5),
        "March pay was wrong (attempt A)",
        "actor",
    )
    .await
    .unwrap();
    add_employment_to_correction_run(&pool, &run_a, &employment_id, Some(&target), "actor")
        .await
        .unwrap();
    assert_eq!(
        calculate_payroll_run(&pool, &run_a, "calculator")
            .await
            .unwrap(),
        Vec::new()
    );

    let run_b = create_correction_run(
        &pool,
        &employer_id,
        period(),
        date(2026, 6, 6),
        "March pay was wrong (attempt B)",
        "actor",
    )
    .await
    .unwrap();
    add_employment_to_correction_run(&pool, &run_b, &employment_id, Some(&target), "actor")
        .await
        .unwrap();
    assert_eq!(
        calculate_payroll_run(&pool, &run_b, "calculator")
            .await
            .unwrap(),
        Vec::new()
    );

    finalize_payroll_run(&pool, &run_a, "finalizer")
        .await
        .unwrap();
    let result_b = finalize_payroll_run(&pool, &run_b, "finalizer").await;

    assert_eq!(
        result_b,
        Err(PayrollAppError::CorrectionTargetAlreadyReplaced(target))
    );
    assert_eq!(
        run_status(&pool, &run_a).await,
        "finalized",
        "the first to finalize wins"
    );
    assert_eq!(
        run_status(&pool, &run_b).await,
        "calculated",
        "the loser is left exactly as it was, not finalized"
    );
}

// ---- Null lineage: exactly two legitimate cases (§4.8) ----

#[sqlx::test]
async fn null_lineage_is_legitimate_after_a_reasoned_removal_from_the_finalized_ordinary_run(
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
    let ordinary_run_id =
        create_ordinary_payroll_run(&pool, &employer_id, period(), date(2026, 4, 5), "actor")
            .await
            .unwrap();
    remove_employment_from_run(
        &pool,
        &ordinary_run_id,
        &employment_id,
        "on unpaid leave",
        "actor",
    )
    .await
    .unwrap();
    assert_eq!(
        calculate_payroll_run(&pool, &ordinary_run_id, "calculator")
            .await
            .unwrap(),
        Vec::new(),
        "a run with no active members reaches Calculated vacuously"
    );
    finalize_payroll_run(&pool, &ordinary_run_id, "finalizer")
        .await
        .unwrap();

    let run_id = create_correction_run(
        &pool,
        &employer_id,
        period(),
        date(2026, 6, 5),
        "removal was a mistake; pay March after all",
        "actor",
    )
    .await
    .unwrap();
    add_employment_to_correction_run(&pool, &run_id, &employment_id, None, "actor")
        .await
        .unwrap();
    assert_eq!(
        calculate_payroll_run(&pool, &run_id, "calculator")
            .await
            .unwrap(),
        Vec::new()
    );

    let outcome = finalize_payroll_run(&pool, &run_id, "finalizer")
        .await
        .unwrap();

    assert_eq!(outcome.finalized.len(), 1);
}

#[sqlx::test]
async fn null_lineage_is_legitimate_when_the_employment_was_never_a_member(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    // March's Ordinary run happens first, for a colleague, and finalizes.
    // Only then is this Employment created — with a backdated start date —
    // so it was never auto-proposed into that run and §4.6 forbids a second
    // one. That is §4.8's second null-lineage case.
    let colleague_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-2",
        Money::from_cents(1200000).unwrap(),
    )
    .await;
    finalize_march(&pool, &employer_id, &colleague_id).await;

    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;

    let run_id = create_correction_run(
        &pool,
        &employer_id,
        period(),
        date(2026, 6, 5),
        "onboarded after March's run already ran",
        "actor",
    )
    .await
    .unwrap();
    add_employment_to_correction_run(&pool, &run_id, &employment_id, None, "actor")
        .await
        .unwrap();
    assert_eq!(
        calculate_payroll_run(&pool, &run_id, "calculator")
            .await
            .unwrap(),
        Vec::new()
    );

    let outcome = finalize_payroll_run(&pool, &run_id, "finalizer")
        .await
        .unwrap();

    assert_eq!(outcome.finalized.len(), 1);
}

#[sqlx::test]
async fn null_lineage_is_refused_for_an_employment_that_was_an_unremoved_finalized_member(
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
    // Finalized normally, live, never removed — there is nothing for a null
    // target to legitimately mean here.
    finalize_march(&pool, &employer_id, &employment_id).await;

    let run_id = create_correction_run(
        &pool,
        &employer_id,
        period(),
        date(2026, 6, 5),
        "should have named the target instead",
        "actor",
    )
    .await
    .unwrap();
    add_employment_to_correction_run(&pool, &run_id, &employment_id, None, "actor")
        .await
        .unwrap();
    assert_eq!(
        calculate_payroll_run(&pool, &run_id, "calculator")
            .await
            .unwrap(),
        Vec::new()
    );

    let result = finalize_payroll_run(&pool, &run_id, "finalizer").await;

    assert_eq!(
        result,
        Err(PayrollAppError::CorrectionLineageNotLegitimate {
            employment_id,
            period: period(),
        })
    );
}

/// §4.8 sets lineage *exactly* when a reversed predecessor exists, so the
/// biconditional bites from the other side too: once a null-lineage
/// correction has itself been reversed, the next correction must name it.
/// Letting a second null target through would leave the first replacement
/// unreplaced forever and fork the chain ADR-0015 keeps linear.
#[sqlx::test]
async fn null_lineage_is_refused_while_a_reversed_predecessor_is_unreplaced(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let colleague_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-2",
        Money::from_cents(1200000).unwrap(),
    )
    .await;
    finalize_march(&pool, &employer_id, &colleague_id).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;

    // A legitimate null-lineage correction — the Employment was never a
    // member — which is then found wrong in its turn and reversed.
    let first = finalize_a_correction(&pool, &employer_id, &employment_id, None).await;
    reverse_finalized_payroll(&pool, &first, "the backdated pay was wrong too", "actor")
        .await
        .unwrap();

    let run_id = create_correction_run(
        &pool,
        &employer_id,
        period(),
        date(2026, 7, 5),
        "should have named the first correction as its target",
        "actor",
    )
    .await
    .unwrap();
    add_employment_to_correction_run(&pool, &run_id, &employment_id, None, "actor")
        .await
        .unwrap();
    calculate_payroll_run(&pool, &run_id, "calculator")
        .await
        .unwrap();

    let result = finalize_payroll_run(&pool, &run_id, "finalizer").await;

    assert_eq!(
        result,
        Err(
            PayrollAppError::CorrectionLineageOmitsAReversedPredecessor {
                employment_id,
                period: period(),
                finalized_payroll_id: first,
            }
        )
    );
}

/// Both of §4.8's null-lineage cases presuppose the Ordinary run for the
/// period has finalized — they exist because §4.6 forbids a second one, and
/// that only bites once the first is history. A Correction that runs before
/// it would collide with it on the `live_finalized_payroll` primary key.
#[sqlx::test]
async fn null_lineage_is_refused_before_the_ordinary_run_for_the_period_finalizes(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    // No Ordinary run for `period()` exists at all: March has simply not
    // been run yet, and running it is still the right way to pay it.

    let run_id = create_correction_run(
        &pool,
        &employer_id,
        period(),
        date(2026, 6, 5),
        "jumping the gun on March",
        "actor",
    )
    .await
    .unwrap();
    add_employment_to_correction_run(&pool, &run_id, &employment_id, None, "actor")
        .await
        .unwrap();
    calculate_payroll_run(&pool, &run_id, "calculator")
        .await
        .unwrap();

    let result = finalize_payroll_run(&pool, &run_id, "finalizer").await;

    assert_eq!(
        result,
        Err(PayrollAppError::CorrectionLineageNotLegitimate {
            employment_id,
            period: period(),
        })
    );
}

/// §4.8's two null-lineage cases are both statements about the *Ordinary*
/// run, and a second null-lineage Correction satisfies them both again: the
/// removal is still reasoned, and the first Correction's own record was
/// never reversed, so no predecessor is waiting to be named. Only the
/// liveness row (§6.2) records that the period has since been paid.
///
/// Refused as a domain refusal naming the way forward — reverse the live
/// record and name it — rather than as the `live_finalized_payroll` primary
/// key violation that would otherwise be the first thing to notice.
#[sqlx::test]
async fn a_second_null_lineage_correction_for_an_already_paid_period_is_refused(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let ordinary_run_id =
        create_ordinary_payroll_run(&pool, &employer_id, period(), date(2026, 4, 5), "actor")
            .await
            .unwrap();
    remove_employment_from_run(
        &pool,
        &ordinary_run_id,
        &employment_id,
        "on unpaid leave",
        "actor",
    )
    .await
    .unwrap();
    calculate_payroll_run(&pool, &ordinary_run_id, "calculator")
        .await
        .unwrap();
    finalize_payroll_run(&pool, &ordinary_run_id, "finalizer")
        .await
        .unwrap();

    // Both Corrections are drafted and calculated before either finalizes,
    // so neither can have seen the other's record when it was added.
    let mut run_ids = Vec::new();
    for _ in 0..2 {
        let run_id = create_correction_run(
            &pool,
            &employer_id,
            period(),
            date(2026, 6, 5),
            "the removal was a mistake; pay March after all",
            "actor",
        )
        .await
        .unwrap();
        add_employment_to_correction_run(&pool, &run_id, &employment_id, None, "actor")
            .await
            .unwrap();
        calculate_payroll_run(&pool, &run_id, "calculator")
            .await
            .unwrap();
        run_ids.push(run_id);
    }

    let first = finalize_payroll_run(&pool, &run_ids[0], "finalizer")
        .await
        .unwrap();
    let second = finalize_payroll_run(&pool, &run_ids[1], "finalizer").await;

    assert_eq!(
        second,
        Err(PayrollAppError::CorrectionPeriodAlreadyHasALivePayroll {
            employment_id: employment_id.clone(),
            period: period(),
        })
    );
    // The refused run left nothing behind, and March is still paid once.
    assert_eq!(
        live_finalized_payroll_id(&pool, &employment_id, period().end()).await,
        Some(first.finalized[0].1.to_string())
    );
    assert_eq!(
        finalized_payroll_count(&pool, &employment_id).await,
        1,
        "a refused finalization writes no history"
    );
    assert_eq!(run_status(&pool, &run_ids[1]).await, "calculated");
}

/// Two null-lineage Corrections finalizing at the same moment: exactly one
/// wins, and the loser is told the same thing the sequential case is told.
///
/// Which guard fires is deliberately not asserted. The read in
/// `verify_null_lineage_is_legitimate` sees the winner's liveness row only
/// once it has committed; before that, the `live_finalized_payroll` primary
/// key is what decides (§5.4, §6.2) — the same shape as two Corrections
/// naming one target, and the reason both are read back as one refusal.
#[sqlx::test]
async fn two_null_lineage_corrections_finalizing_at_once_end_with_only_the_first_finalized(
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
    let ordinary_run_id =
        create_ordinary_payroll_run(&pool, &employer_id, period(), date(2026, 4, 5), "actor")
            .await
            .unwrap();
    remove_employment_from_run(
        &pool,
        &ordinary_run_id,
        &employment_id,
        "on unpaid leave",
        "actor",
    )
    .await
    .unwrap();
    calculate_payroll_run(&pool, &ordinary_run_id, "calculator")
        .await
        .unwrap();
    finalize_payroll_run(&pool, &ordinary_run_id, "finalizer")
        .await
        .unwrap();

    let mut run_ids = Vec::new();
    for _ in 0..2 {
        let run_id = create_correction_run(
            &pool,
            &employer_id,
            period(),
            date(2026, 6, 5),
            "the removal was a mistake; pay March after all",
            "actor",
        )
        .await
        .unwrap();
        add_employment_to_correction_run(&pool, &run_id, &employment_id, None, "actor")
            .await
            .unwrap();
        calculate_payroll_run(&pool, &run_id, "calculator")
            .await
            .unwrap();
        run_ids.push(run_id);
    }

    let (first, second) = tokio::join!(
        finalize_payroll_run(&pool, &run_ids[0], "finalizer"),
        finalize_payroll_run(&pool, &run_ids[1], "finalizer"),
    );

    let refusal = match (first, second) {
        (Ok(_), Err(refusal)) | (Err(refusal), Ok(_)) => refusal,
        (first, second) => panic!("exactly one must finalize, got {first:?} and {second:?}"),
    };
    assert_eq!(
        refusal,
        PayrollAppError::CorrectionPeriodAlreadyHasALivePayroll {
            employment_id: employment_id.clone(),
            period: period(),
        }
    );
    assert_eq!(
        finalized_payroll_count(&pool, &employment_id).await,
        1,
        "March is paid exactly once"
    );
}

/// ADR-0015's chain, walked twice through the public API:
/// `F1 → reversed → F2 (replaces F1) → reversed → F3 (replaces F2)`.
#[sqlx::test]
async fn repeated_corrections_form_a_chain_rather_than_a_tree(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;

    let f1 = finalize_and_reverse_march(&pool, &employer_id, &employment_id).await;
    let f2 = finalize_a_correction(&pool, &employer_id, &employment_id, Some(&f1)).await;
    reverse_finalized_payroll(&pool, &f2, "the first correction was wrong too", "actor")
        .await
        .unwrap();
    let f3 = finalize_a_correction(&pool, &employer_id, &employment_id, Some(&f2)).await;

    // Each link names exactly one predecessor, and F1 is named once only.
    let lineage: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT id::text, replaces_finalized_payroll_id::text
         FROM finalized_payroll WHERE employment_id = $1 ORDER BY finalized_at",
    )
    .bind(employment_id.as_str())
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(
        lineage,
        vec![
            (f1.to_string(), None),
            (f2.to_string(), Some(f1.to_string())),
            (f3.to_string(), Some(f2.to_string())),
        ]
    );
    assert_eq!(
        live_finalized_payroll_id(&pool, &employment_id, period().end()).await,
        Some(f3.to_string())
    );
}

// ---- Earning pre-population (§4.5d, §6.3, §9.1) ----

#[sqlx::test]
async fn earnings_are_prepopulated_from_the_reversed_targets_frozen_snapshot(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let ordinary_run_id =
        create_ordinary_payroll_run(&pool, &employer_id, period(), date(2026, 4, 5), "actor")
            .await
            .unwrap();
    set_run_earnings(
        &pool,
        &ordinary_run_id,
        &employment_id,
        vec![Earning::TaxableAllowance(Money::from_cents(20000).unwrap())],
    )
    .await
    .unwrap();
    calculate_payroll_run(&pool, &ordinary_run_id, "calculator")
        .await
        .unwrap();
    let outcome = finalize_payroll_run(&pool, &ordinary_run_id, "finalizer")
        .await
        .unwrap();
    let target = outcome.finalized[0].1.clone();
    reverse_finalized_payroll(&pool, &target, "March salary was wrong", "actor")
        .await
        .unwrap();

    let run_id = create_correction_run(
        &pool,
        &employer_id,
        period(),
        date(2026, 6, 5),
        "March pay was wrong",
        "actor",
    )
    .await
    .unwrap();

    let pre_population =
        add_employment_to_correction_run(&pool, &run_id, &employment_id, Some(&target), "actor")
            .await
            .unwrap();

    assert_eq!(
        pre_population,
        EarningPrePopulation::FromTarget { count: 1 }
    );
    let earning_json: serde_json::Value = sqlx::query_scalar(
        "SELECT earning_json FROM payroll_run_earning
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        earning_json,
        serde_json::to_value(Earning::TaxableAllowance(Money::from_cents(20000).unwrap())).unwrap()
    );
}

#[sqlx::test]
async fn no_target_prepopulates_no_earnings(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let run_id = create_correction_run(
        &pool,
        &employer_id,
        period(),
        date(2026, 6, 5),
        "never a member",
        "actor",
    )
    .await
    .unwrap();

    let pre_population =
        add_employment_to_correction_run(&pool, &run_id, &employment_id, None, "actor")
            .await
            .unwrap();

    assert_eq!(pre_population, EarningPrePopulation::NoTarget);
}

/// A snapshot stamped with a version this build *does* read, whose frozen
/// input has no `earnings` field at all, is unreadable in the same way and
/// says so in the same words. A `PayrollInput` at this version always
/// serializes the field, so its absence says the shape is not the one this
/// build reads — and answering "no allowances" to that would be the guess
/// §9.1 forbids, dressed up as a degradation.
#[sqlx::test]
async fn a_snapshot_missing_its_earnings_field_degrades_to_no_earnings_and_says_so(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let target = finalize_and_reverse_march(&pool, &employer_id, &employment_id).await;
    sqlx::query("UPDATE finalized_payroll SET payroll_input_json = '{}' WHERE id = $1::uuid")
        .bind(target.as_str())
        .execute(&pool)
        .await
        .unwrap();

    let run_id = create_correction_run(
        &pool,
        &employer_id,
        period(),
        date(2026, 6, 5),
        "March pay was wrong",
        "actor",
    )
    .await
    .unwrap();

    let pre_population =
        add_employment_to_correction_run(&pool, &run_id, &employment_id, Some(&target), "actor")
            .await
            .unwrap();

    assert_eq!(
        pre_population,
        EarningPrePopulation::UnreadableSnapshot {
            schema_version: SNAPSHOT_SCHEMA_VERSION
        }
    );
}

#[sqlx::test]
async fn an_unreadable_snapshot_schema_degrades_to_no_earnings_and_says_so(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let target = finalize_and_reverse_march(&pool, &employer_id, &employment_id).await;
    // A future schema shape this build cannot read — `snapshot_schema_version`
    // is stated explicitly at finalization (§9.1), never left to a default,
    // so this is the one honest way to fabricate a not-yet-invented version
    // in a test.
    sqlx::query("UPDATE finalized_payroll SET snapshot_schema_version = $1 WHERE id = $2::uuid")
        .bind(SNAPSHOT_SCHEMA_VERSION + 1)
        .bind(target.as_str())
        .execute(&pool)
        .await
        .unwrap();

    let run_id = create_correction_run(
        &pool,
        &employer_id,
        period(),
        date(2026, 6, 5),
        "March pay was wrong",
        "actor",
    )
    .await
    .unwrap();

    let pre_population =
        add_employment_to_correction_run(&pool, &run_id, &employment_id, Some(&target), "actor")
            .await
            .unwrap();

    assert_eq!(
        pre_population,
        EarningPrePopulation::UnreadableSnapshot {
            schema_version: SNAPSHOT_SCHEMA_VERSION + 1
        }
    );
    let earning_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payroll_run_earning
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(earning_count, 0);
}

// ---- ActionLog (§10) ----

#[sqlx::test]
async fn the_added_action_log_entry_carries_the_correction_reason_and_the_target(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let target = finalize_and_reverse_march(&pool, &employer_id, &employment_id).await;
    let run_id = create_correction_run(
        &pool,
        &employer_id,
        period(),
        date(2026, 6, 5),
        "March pay was wrong",
        "actor",
    )
    .await
    .unwrap();

    add_employment_to_correction_run(&pool, &run_id, &employment_id, Some(&target), "actor")
        .await
        .unwrap();

    let context = action_log_context(
        &pool,
        "employment_added_to_correction_run",
        employment_id.as_str(),
    )
    .await;
    assert_eq!(context["correction_reason"], "March pay was wrong");
    assert_eq!(context["replaces_finalized_payroll_id"], target.as_str());
}

// ---- §6.5: the rest of PayrollInput comes from current master data ----

#[sqlx::test]
async fn a_corrected_compensation_terms_reaches_the_replacement_calculation(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let target = finalize_and_reverse_march(&pool, &employer_id, &employment_id).await;

    // Master data is corrected after the reversal — the true March rate, per
    // §6.5's "split the row" repair, through `CorrectCompensationTerms`
    // (§12, issue #36) rather than raw SQL. March's `FinalizedPayroll` was
    // just reversed, so it is no longer Live and the correction diverges from
    // nothing: `&[]` is the whole acknowledgement it owes.
    let diverging = correct_compensation_terms(
        &pool,
        &employment_id,
        period().start(),
        period().start(),
        Money::from_cents(1600000).unwrap(),
        &[],
        "March rate captured wrong",
        "actor",
    )
    .await
    .unwrap();
    assert_eq!(
        diverging,
        Vec::new(),
        "the reversed March record is not Live, so this correction diverges from nothing"
    );

    let run_id = create_correction_run(
        &pool,
        &employer_id,
        period(),
        date(2026, 6, 5),
        "March salary was captured wrong",
        "actor",
    )
    .await
    .unwrap();
    add_employment_to_correction_run(&pool, &run_id, &employment_id, Some(&target), "actor")
        .await
        .unwrap();
    assert_eq!(
        calculate_payroll_run(&pool, &run_id, "calculator")
            .await
            .unwrap(),
        Vec::new()
    );

    let outcome = finalize_payroll_run(&pool, &run_id, "finalizer")
        .await
        .unwrap();

    let taxable_remuneration: i64 = sqlx::query_scalar(
        "SELECT taxable_remuneration FROM finalized_payroll WHERE id = $1::uuid",
    )
    .bind(outcome.finalized[0].1.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        taxable_remuneration, 1600000,
        "the replacement must reflect the corrected BasicPay, not the reversed figure"
    );
}

// ---- §7.4 / §6.4: own period only, later periods untouched, warned about ----

#[sqlx::test]
async fn a_correction_checks_its_own_period_only_and_warns_about_later_finalized_periods(
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
    let target = finalize_and_reverse_march(&pool, &employer_id, &employment_id).await;

    // April is finalized against the (reversed) March's year-to-date.
    let april_run_id = create_ordinary_payroll_run(
        &pool,
        &employer_id,
        next_period(),
        date(2026, 5, 5),
        "actor",
    )
    .await
    .unwrap();
    calculate_payroll_run(&pool, &april_run_id, "calculator")
        .await
        .unwrap();
    let april_outcome = finalize_payroll_run(&pool, &april_run_id, "finalizer")
        .await
        .unwrap();
    let april_finalized_payroll_id = april_outcome.finalized[0].1.clone();
    let april_row_before = finalized_payroll_row(&pool, &april_finalized_payroll_id).await;

    let run_id = create_correction_run(
        &pool,
        &employer_id,
        period(),
        date(2026, 6, 5),
        "March pay was wrong",
        "actor",
    )
    .await
    .unwrap();
    add_employment_to_correction_run(&pool, &run_id, &employment_id, Some(&target), "actor")
        .await
        .unwrap();
    assert_eq!(
        calculate_payroll_run(&pool, &run_id, "calculator")
            .await
            .unwrap(),
        Vec::new()
    );

    let outcome = finalize_payroll_run(&pool, &run_id, "finalizer")
        .await
        .unwrap();

    assert_eq!(outcome.later_finalized_periods, vec![next_period()]);

    // April's whole row is untouched — not only the figures year-to-date
    // reads, but the frozen snapshots that explain them.
    assert_eq!(
        april_row_before,
        finalized_payroll_row(&pool, &april_finalized_payroll_id).await,
        "later periods are never rewritten"
    );
    assert_eq!(
        live_finalized_payroll_id(&pool, &employment_id, next_period().end()).await,
        Some(april_finalized_payroll_id.as_str().to_string()),
        "April's liveness is untouched"
    );
}

// ---- §8: year-to-date sees the replacement, never the original ----

#[sqlx::test]
async fn year_to_date_built_after_replacement_sees_the_replacement_not_the_original(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let target = finalize_and_reverse_march(&pool, &employer_id, &employment_id).await;
    sqlx::query(
        "UPDATE compensation_terms SET basic_pay = $1
         WHERE employment_id = $2 AND effective_from = $3",
    )
    .bind(Money::from_cents(1600000).unwrap().cents())
    .bind(employment_id.as_str())
    .bind(period().start())
    .execute(&pool)
    .await
    .unwrap();
    let run_id = create_correction_run(
        &pool,
        &employer_id,
        period(),
        date(2026, 6, 5),
        "March salary was captured wrong",
        "actor",
    )
    .await
    .unwrap();
    add_employment_to_correction_run(&pool, &run_id, &employment_id, Some(&target), "actor")
        .await
        .unwrap();
    calculate_payroll_run(&pool, &run_id, "calculator")
        .await
        .unwrap();
    let outcome = finalize_payroll_run(&pool, &run_id, "finalizer")
        .await
        .unwrap();
    let replacement_id = outcome.finalized[0].1.clone();

    let ytd = payroll_app::build_year_to_date_context(&pool, &employment_id, next_period().end())
        .await
        .unwrap();

    assert_eq!(
        ytd.prior_taxable_remuneration(),
        Money::from_cents(1600000).unwrap(),
        "year-to-date must pick up the replacement's corrected figure, not the reversed original"
    );
    assert_eq!(
        live_finalized_payroll_id(&pool, &employment_id, period().end()).await,
        Some(replacement_id.as_str().to_string())
    );
}
