//! Proves the use case issue #31 introduces: `finalize_payroll_run` —
//! `docs/domain/payroll-run-persistence.md` §5 and §9 — reached through the
//! public API, never raw SQL, except where a scenario needs data the public
//! API cannot yet produce (a tampered stored calculation, a corrected
//! `CompensationTerms` row) or where the thing under test is a database
//! guarantee rather than a use case (the concurrency test).
//!
//! Every fixture uses an Employment's own first payable period (Deep
//! Instructions on issue #31): the Ordinary preceding-period check (§5.3
//! step 3, §7) is not built yet, and these tests must stay valid once it is.

use chrono::NaiveDate;
use payroll::{
    EmployerId, EmploymentId, Money, PayPeriod, PeriodEndDay, PersonId, PriorEmployment, TaxYear,
    UnsupportedDeductionStatus, ruleset_for,
};
use payroll_app::{
    PayrollAppError, PayrollRunId, SALT_VERSION, calculate_payroll_run, create_employer,
    create_employment, create_ordinary_payroll_run, declare_prior_employment,
    declare_unsupported_deduction_status, finalize_payroll_run, record_compensation_terms,
};
use sqlx::{Acquire, PgPool, Row};
use tokio::sync::oneshot;

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn monthly_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::LastDayOfMonth)
}

/// March 2026 — the calendar month `monthly_schedule()` generates. Every
/// fixture below gives the Employment a `start_date` of this period's own
/// start, so this is its first payable period and no `OpeningBalance` or
/// earlier resolved period is required (§7.1 branch 1).
fn period() -> PayPeriod {
    PayPeriod::new(date(2026, 3, 1), date(2026, 3, 31)).unwrap()
}

/// The `PayPeriod` immediately after `period()`, under the same schedule —
/// used by the year-to-date pickup test.
fn next_period() -> PayPeriod {
    PayPeriod::new(date(2026, 4, 1), date(2026, 4, 30)).unwrap()
}

async fn an_employer(pool: &PgPool) -> EmployerId {
    create_employer(pool, monthly_schedule(), "actor")
        .await
        .unwrap()
}

/// An Employment starting on `period()`'s own first day, with every fact
/// `calculate` needs already on record: `CompensationTerms` effective from
/// that same day, a confirmed absence of `PriorEmployment`, and a confirmed
/// absence of unsupported deductions. `period()` is this Employment's first
/// payable period, so no `OpeningBalance` is required (§7.1 branch 1).
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

/// Creates a March Ordinary run and calculates it to `Calculated`.
async fn a_calculated_run(pool: &PgPool, employer_id: &EmployerId) -> PayrollRunId {
    let run_id =
        create_ordinary_payroll_run(pool, employer_id, period(), date(2026, 4, 5), "actor")
            .await
            .unwrap();
    let refusals = calculate_payroll_run(pool, &run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(refusals, Vec::new(), "the run must reach Calculated");
    run_id
}

async fn run_status(pool: &PgPool, run_id: &PayrollRunId) -> String {
    sqlx::query_scalar("SELECT status FROM payroll_run WHERE id = $1::uuid")
        .bind(run_id.as_str())
        .fetch_one(pool)
        .await
        .unwrap()
}

struct FinalizedPayrollRow {
    input_json: serde_json::Value,
    rules_json: serde_json::Value,
    calculation_json: serde_json::Value,
    taxable_remuneration_cents: i64,
    paye_cents: i64,
    paye_table_id: String,
    ssc_rules_id: String,
    salt_version: String,
    snapshot_schema_version: i32,
    finalized_by: String,
    replaces_finalized_payroll_id: Option<String>,
}

async fn finalized_payroll_row(
    pool: &PgPool,
    run_id: &PayrollRunId,
    employment_id: &EmploymentId,
) -> Option<FinalizedPayrollRow> {
    sqlx::query(
        "SELECT payroll_input_json, payroll_rules_json, payroll_calculation_json,
                taxable_remuneration, paye, paye_table_id, ssc_rules_id, salt_version,
                snapshot_schema_version, finalized_by, replaces_finalized_payroll_id::text
         FROM finalized_payroll
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_optional(pool)
    .await
    .unwrap()
    .map(|row| FinalizedPayrollRow {
        input_json: row.get(0),
        rules_json: row.get(1),
        calculation_json: row.get(2),
        taxable_remuneration_cents: row.get(3),
        paye_cents: row.get(4),
        paye_table_id: row.get(5),
        ssc_rules_id: row.get(6),
        salt_version: row.get(7),
        snapshot_schema_version: row.get(8),
        finalized_by: row.get(9),
        replaces_finalized_payroll_id: row.get(10),
    })
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

async fn action_log_count(pool: &PgPool, action_type: &str, target_id: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM action_log_entry WHERE action_type = $1 AND target_id = $2",
    )
    .bind(action_type)
    .bind(target_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn working_calculation_input_json(
    pool: &PgPool,
    run_id: &PayrollRunId,
    employment_id: &EmploymentId,
) -> serde_json::Value {
    sqlx::query_scalar(
        "SELECT payroll_input_json FROM working_payroll_calculation
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(pool)
    .await
    .unwrap()
}

// ---- The tracer bullet (§14): freezes the complete snapshot ----

#[sqlx::test]
async fn finalizing_a_calculated_run_freezes_the_complete_snapshot_and_marks_the_run_finalized(
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
    let run_id = a_calculated_run(&pool, &employer_id).await;

    finalize_payroll_run(&pool, &run_id, "finalizer")
        .await
        .unwrap();

    assert_eq!(run_status(&pool, &run_id).await, "finalized");

    let expected_rules = ruleset_for(period().end()).unwrap();
    let row = finalized_payroll_row(&pool, &run_id, &employment_id)
        .await
        .expect("a FinalizedPayroll row must exist");
    assert_eq!(row.paye_table_id, expected_rules.paye_table().id().as_str());
    assert_eq!(row.ssc_rules_id, expected_rules.ssc_ruleset().id().as_str());
    assert_eq!(row.salt_version, SALT_VERSION);
    assert_eq!(row.snapshot_schema_version, 1);
    assert_eq!(row.finalized_by, "finalizer");
    assert_eq!(row.replaces_finalized_payroll_id, None);
    assert_eq!(row.taxable_remuneration_cents, 1500000);
    assert_eq!(
        row.paye_cents,
        row.calculation_json["paye"]["amount"].as_i64().unwrap()
    );
    assert_eq!(
        row.input_json["earnings"],
        serde_json::json!([]),
        "the complete PayrollInput is frozen, not just the figures"
    );
    assert_eq!(
        row.rules_json["ssc_ruleset"]["id"],
        serde_json::json!("ssc-2025-03")
    );

    let live_id = live_finalized_payroll_id(&pool, &employment_id, period().end())
        .await
        .expect("finalizing must insert a liveness row");
    let finalized_id: String = sqlx::query_scalar(
        "SELECT id::text FROM finalized_payroll WHERE payroll_run_id = $1::uuid",
    )
    .bind(run_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(live_id, finalized_id);

    assert_eq!(
        action_log_count(&pool, "payroll_finalized", run_id.as_str()).await,
        1
    );
}

#[sqlx::test]
async fn a_run_with_no_active_members_finalizes_vacuously(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let run_id = a_calculated_run(&pool, &employer_id).await;

    finalize_payroll_run(&pool, &run_id, "finalizer")
        .await
        .unwrap();

    assert_eq!(run_status(&pool, &run_id).await, "finalized");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM finalized_payroll WHERE payroll_run_id = $1::uuid",
    )
    .bind(run_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        action_log_count(&pool, "payroll_finalized", run_id.as_str()).await,
        1
    );
}

#[sqlx::test]
async fn a_later_periods_year_to_date_picks_up_the_finalized_figures(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let march_run_id = a_calculated_run(&pool, &employer_id).await;
    finalize_payroll_run(&pool, &march_run_id, "finalizer")
        .await
        .unwrap();
    let march = finalized_payroll_row(&pool, &march_run_id, &employment_id)
        .await
        .unwrap();

    let april_run_id = create_ordinary_payroll_run(
        &pool,
        &employer_id,
        next_period(),
        date(2026, 5, 5),
        "actor",
    )
    .await
    .unwrap();
    let refusals = calculate_payroll_run(&pool, &april_run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(refusals, Vec::new());

    let april_input = working_calculation_input_json(&pool, &april_run_id, &employment_id).await;
    assert_eq!(
        april_input["year_to_date"]["prior_taxable_remuneration"],
        serde_json::json!(march.taxable_remuneration_cents)
    );
    assert_eq!(
        april_input["year_to_date"]["prior_paye"],
        serde_json::json!(march.paye_cents)
    );
}

// ---- Lifecycle refusals ----

#[sqlx::test]
async fn finalizing_a_missing_run_is_refused(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let run_id = a_calculated_run(&pool, &employer_id).await;
    sqlx::query("DELETE FROM payroll_run WHERE id = $1::uuid")
        .bind(run_id.as_str())
        .execute(&pool)
        .await
        .unwrap();

    let result = finalize_payroll_run(&pool, &run_id, "finalizer").await;

    assert_eq!(result, Err(PayrollAppError::PayrollRunNotFound(run_id)));
}

#[sqlx::test]
async fn finalizing_a_draft_run_is_refused(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let run_id =
        create_ordinary_payroll_run(&pool, &employer_id, period(), date(2026, 4, 5), "actor")
            .await
            .unwrap();

    let result = finalize_payroll_run(&pool, &run_id, "finalizer").await;

    assert_eq!(
        result,
        Err(PayrollAppError::PayrollRunNotCalculated(run_id))
    );
}

#[sqlx::test]
async fn finalizing_an_already_finalized_run_is_refused(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let run_id = a_calculated_run(&pool, &employer_id).await;
    finalize_payroll_run(&pool, &run_id, "finalizer")
        .await
        .unwrap();

    let result = finalize_payroll_run(&pool, &run_id, "finalizer").await;

    assert_eq!(
        result,
        Err(PayrollAppError::PayrollRunAlreadyFinalized(run_id))
    );
}

/// §5.3 step 2 verifies the run's **kind** as well as its status, and this
/// use case implements the Ordinary column of step 3 only. A Correction run
/// must also carry `replaces_finalized_payroll_id` into its
/// `FinalizedPayroll` (§9) and check its target is reversed and not live
/// (§4.8) — so finalizing one here would write a replacement with the null
/// lineage §9 reserves for two other cases entirely.
///
/// No use case creates a Correction run yet, so the run's kind is changed
/// directly. The run is left empty because a Correction run may hold at most
/// one member (§4.8, migration 0016's trigger).
#[sqlx::test]
async fn finalizing_a_correction_run_is_refused(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let run_id = a_calculated_run(&pool, &employer_id).await;
    sqlx::query(
        "UPDATE payroll_run SET kind = 'correction', correction_reason = 'a corrected March'
         WHERE id = $1::uuid",
    )
    .bind(run_id.as_str())
    .execute(&pool)
    .await
    .unwrap();

    let result = finalize_payroll_run(&pool, &run_id, "finalizer").await;

    assert_eq!(
        result,
        Err(PayrollAppError::PayrollRunIsNotOrdinary(run_id.clone()))
    );
    assert_eq!(run_status(&pool, &run_id).await, "calculated");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM finalized_payroll")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        action_log_count(&pool, "payroll_finalized", run_id.as_str()).await,
        0,
        "a refused finalization writes no audit entry either"
    );
}

// ---- Three-way finalization equality (§5.2, §14 tests 30-33) ----

/// The tempting simplification is comparing the `PayrollCalculation` alone.
/// This tampers the *stored* calculation directly — the only way to make a
/// recomputed `PayrollCalculation` disagree while its own inputs and rules
/// have not moved, since `calculate` is otherwise deterministic in them.
#[sqlx::test]
async fn finalization_refuses_when_the_recomputed_calculation_differs(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let run_id = a_calculated_run(&pool, &employer_id).await;

    let calculation_json: serde_json::Value = sqlx::query_scalar(
        "SELECT payroll_calculation_json FROM working_payroll_calculation
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut tampered: payroll::PayrollCalculation =
        serde_json::from_value(calculation_json).unwrap();
    tampered.net_pay = tampered
        .net_pay
        .checked_add(Money::from_cents(1).unwrap())
        .unwrap();
    sqlx::query(
        "UPDATE working_payroll_calculation SET payroll_calculation_json = $1
         WHERE payroll_run_id = $2::uuid AND employment_id = $3",
    )
    .bind(serde_json::to_value(&tampered).unwrap())
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .execute(&pool)
    .await
    .unwrap();

    let result = finalize_payroll_run(&pool, &run_id, "finalizer").await;

    assert!(
        matches!(
            result,
            Err(PayrollAppError::FinalizationCalculationMismatch { employment_id: ref id, .. })
                if *id == employment_id
        ),
        "expected a FinalizationCalculationMismatch, got {result:?}"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM finalized_payroll")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "a refused finalization must leave no partial history"
    );
    assert_eq!(run_status(&pool, &run_id).await, "calculated");
}

/// A `CompensationTerms.effective_from` corrected without touching
/// `BasicPay` (§5.2's own example): the reassembled `PayrollInput` differs
/// while the recomputed `PayrollCalculation` stays byte-identical, because
/// `calculate` never reads `effective_from` into the monetary result. The
/// public API has no "correct a CompensationTerms row" use case yet (§6.5
/// is a separate, later ticket), so the row is corrected directly.
#[sqlx::test]
async fn finalization_refuses_when_the_reassembled_input_differs_while_the_calculation_is_byte_identical(
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
    let run_id = a_calculated_run(&pool, &employer_id).await;
    let stored_calculation_json: serde_json::Value = sqlx::query_scalar(
        "SELECT payroll_calculation_json FROM working_payroll_calculation
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();

    // Still a valid PayPeriod start under `monthly_schedule()`, still on or
    // before the period being paid, and the row is still the only one — so
    // BasicPay and everything `calculate` derives from it are untouched.
    sqlx::query(
        "UPDATE compensation_terms SET effective_from = '2026-01-01' WHERE employment_id = $1",
    )
    .bind(employment_id.as_str())
    .execute(&pool)
    .await
    .unwrap();

    let result = finalize_payroll_run(&pool, &run_id, "finalizer").await;

    assert!(
        matches!(
            result,
            Err(PayrollAppError::FinalizationInputMismatch { employment_id: ref id, .. })
                if *id == employment_id
        ),
        "expected a FinalizationInputMismatch, got {result:?}"
    );
    // §5.2 and ADR-0010: naming which of the three differed is half the
    // requirement; the refusal must also say what changed inside it, or it
    // is one users learn to click past.
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("effective_from"),
        "the refusal must name the field that changed, got {message}"
    );
    assert!(
        message.contains("2026-01-01"),
        "the refusal must show what it changed to, got {message}"
    );

    // The calculation a fresh recompute would produce is still exactly what
    // was stored — proving an output-only comparison would have missed this.
    let recomputed_calculation_json: serde_json::Value = sqlx::query_scalar(
        "SELECT payroll_calculation_json FROM working_payroll_calculation
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored_calculation_json, recomputed_calculation_json);

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM finalized_payroll")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

/// A PAYE band, or here an SSC ruleset id, corrected outside anything this
/// Employee's figures touch (§5.2's other example): the re-resolved
/// `PayrollRules` differ from what was approved while the recomputed
/// `PayrollCalculation` stays byte-identical. The shipped catalogue is fixed
/// Rust constants (`ruleset.rs`), so the *stored* rules are tampered instead
/// — exactly modelling a later release correcting the catalogue between
/// calculation and finalization.
#[sqlx::test]
async fn finalization_refuses_when_the_re_resolved_rules_differ_while_the_calculation_is_byte_identical(
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
    let run_id = a_calculated_run(&pool, &employer_id).await;

    let stored_calculation_json: serde_json::Value = sqlx::query_scalar(
        "SELECT payroll_calculation_json FROM working_payroll_calculation
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();

    // Tampers the stored `PayrollRules` JSON directly, changing only the
    // `SscRulesId` — a field that carries no weight in the arithmetic itself
    // (ADR-0007's "a catalogue mistake now says which catalogue is wrong"),
    // exactly modelling a PAYE band corrected outside the range this
    // Employee reaches: the money is untouched, the frozen rules are not.
    let mut rules_json: serde_json::Value = sqlx::query_scalar(
        "SELECT payroll_rules_json FROM working_payroll_calculation
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    rules_json["ssc_ruleset"]["id"] = serde_json::json!("ssc-2025-03-tampered");
    // The tamper must still round-trip as a real `PayrollRules`, or this
    // test would be proving something about `serde_json::Value` rather than
    // about `finalize_payroll_run`.
    let _: payroll::PayrollRules = serde_json::from_value(rules_json.clone())
        .expect("the tampered JSON must still deserialize as a PayrollRules");

    sqlx::query(
        "UPDATE working_payroll_calculation SET payroll_rules_json = $1
         WHERE payroll_run_id = $2::uuid AND employment_id = $3",
    )
    .bind(rules_json)
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .execute(&pool)
    .await
    .unwrap();

    let result = finalize_payroll_run(&pool, &run_id, "finalizer").await;

    assert!(
        matches!(
            result,
            Err(PayrollAppError::FinalizationRulesMismatch { employment_id: ref id, .. })
                if *id == employment_id
        ),
        "expected a FinalizationRulesMismatch, got {result:?}"
    );

    // The stored calculation was computed under the real ruleset and was
    // never touched by this tamper, so it is still exactly what a fresh
    // recompute under the real ruleset produces.
    let recomputed_calculation_json: serde_json::Value = sqlx::query_scalar(
        "SELECT payroll_calculation_json FROM working_payroll_calculation
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored_calculation_json, recomputed_calculation_json);

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM finalized_payroll")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

/// §5.1: one bad member blocks the whole run. A member that compared
/// cleanly must not finalize either when a *later* member's stored
/// calculation has been tampered with.
///
/// The order matters and is asserted, not assumed: members are rebuilt
/// `ORDER BY employment_id` and ids are UUIDv7, so the Employment created
/// first is compared first. Tampering the one created *second* is what makes
/// this the hard case — a member already compared and found equal, and a
/// refusal after it — rather than a refusal on the very first member, which
/// would pass even if finalization wrote each member as it went.
#[sqlx::test]
async fn a_later_members_mismatch_leaves_no_finalized_payroll_for_the_earlier_one(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let good = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-good",
        Money::from_cents(900000).unwrap(),
    )
    .await;
    let bad = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-bad",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    assert!(
        good.as_str() < bad.as_str(),
        "UUIDv7 ids order by creation, so the tampered member must be rebuilt second"
    );
    let run_id = a_calculated_run(&pool, &employer_id).await;

    let calculation_json: serde_json::Value = sqlx::query_scalar(
        "SELECT payroll_calculation_json FROM working_payroll_calculation
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(bad.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    let mut tampered: payroll::PayrollCalculation =
        serde_json::from_value(calculation_json).unwrap();
    tampered.net_pay = tampered
        .net_pay
        .checked_add(Money::from_cents(1).unwrap())
        .unwrap();
    sqlx::query(
        "UPDATE working_payroll_calculation SET payroll_calculation_json = $1
         WHERE payroll_run_id = $2::uuid AND employment_id = $3",
    )
    .bind(serde_json::to_value(&tampered).unwrap())
    .bind(run_id.as_str())
    .bind(bad.as_str())
    .execute(&pool)
    .await
    .unwrap();

    let result = finalize_payroll_run(&pool, &run_id, "finalizer").await;

    assert!(result.is_err());
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM finalized_payroll")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "the good member must not finalize while the bad one refuses"
    );
    assert!(
        live_finalized_payroll_id(&pool, &good, period().end())
            .await
            .is_none(),
        "no liveness row for the good member either"
    );
}

// ---- Concurrency (§5.4, §14 test 1) ----
//
// `finalize_payroll_run` cannot itself be driven from inside `tokio::spawn`:
// it is built from async functions generic over `impl Acquire<'a, ...>` (so
// they can run standalone or inside a caller's transaction), and that shape
// hits a known rustc/tokio limitation where the spawned future's `Send` bound
// is inferred for one specific lifetime rather than proven for every one
// (rust-lang/rust#100013 and similar) — `cargo build` reports "implementation
// of `Send`/`Acquire` is not general enough" for *any* async fn in this crate
// spawned this way, `calculate_payroll_run` included. So this test proves the
// two halves separately: raw SQL, on two real connections with a controlled
// interleaving, proves the `FOR UPDATE` lock genuinely blocks a second
// transaction rather than merely being present in the SQL; the real
// `finalize_payroll_run`, called twice in the sequence the lock enforces,
// proves that exactly one of the two calls a lock like that could ever admit
// actually succeeds, and that the loser's own refusal — not an
// application-side status check — is what the lock's blocked side sees.
#[sqlx::test]
async fn two_finalizations_of_the_same_run_end_with_exactly_one_success(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let run_id = a_calculated_run(&pool, &employer_id).await;

    // Phase 1: prove the lock itself blocks a second transaction. A raw-SQL
    // connection takes the exact `FOR UPDATE` lock `finalize_payroll_run`
    // opens with, and a second connection racing for the same row is proven
    // to wait rather than proceed.
    let mut holder = pool
        .acquire()
        .await
        .expect("acquire lock-holding connection");
    let mut holder_tx = holder
        .begin()
        .await
        .expect("begin lock-holding transaction");
    sqlx::query_scalar::<_, String>(
        "SELECT status FROM payroll_run WHERE id = $1::uuid FOR UPDATE",
    )
    .bind(run_id.as_str())
    .fetch_one(&mut *holder_tx)
    .await
    .expect("take the row lock");

    let (started_sender, started_receiver) = oneshot::channel();
    let racing_pool = pool.clone();
    let racing_run_id = run_id.clone();
    let racing_task = tokio::spawn(async move {
        let mut conn = racing_pool
            .acquire()
            .await
            .expect("acquire racing connection");
        let mut tx = conn.begin().await.expect("begin racing transaction");
        started_sender.send(()).expect("notify the lock holder");
        sqlx::query_scalar::<_, String>(
            "SELECT status FROM payroll_run WHERE id = $1::uuid FOR UPDATE",
        )
        .bind(racing_run_id.as_str())
        .fetch_one(&mut *tx)
        .await
        .expect("the row lock, once released, is granted here")
    });

    started_receiver.await.expect("racing transaction started");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !racing_task.is_finished(),
        "a second transaction must wait for the first's row lock, exactly as \
         two finalizers of the same run would"
    );
    holder_tx
        .rollback()
        .await
        .expect("release the lock without changing anything");
    let status_seen_after_the_lock_was_released = racing_task.await.expect("join racing task");
    assert_eq!(status_seen_after_the_lock_was_released, "calculated");

    // Phase 2: the lock is what a real finalizer takes first, so two real
    // calls made in the order the lock would enforce show what its winner
    // and its loser each see — success once, then the loser's own refusal.
    finalize_payroll_run(&pool, &run_id, "finalizer-a")
        .await
        .unwrap();
    let second_call = finalize_payroll_run(&pool, &run_id, "finalizer-b").await;
    assert_eq!(
        second_call,
        Err(PayrollAppError::PayrollRunAlreadyFinalized(run_id.clone()))
    );

    let live_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM live_finalized_payroll WHERE employment_id = $1 AND period_end = $2",
    )
    .bind(employment_id.as_str())
    .bind(period().end())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(live_count, 1, "no duplicate live history");

    let finalized_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM finalized_payroll WHERE employment_id = $1")
            .bind(employment_id.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(finalized_count, 1, "no duplicate FinalizedPayroll row");
}
