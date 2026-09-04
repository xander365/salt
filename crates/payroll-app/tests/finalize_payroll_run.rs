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
    PayrollAppError, PayrollRunId, SALT_VERSION, SNAPSHOT_SCHEMA_VERSION, SaltDatabase,
    calculate_payroll_run, create_employer, create_employment, create_ordinary_payroll_run,
    declare_prior_employment, declare_unsupported_deduction_status, finalize_payroll_run,
    record_compensation_terms, reverse_finalized_payroll,
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

async fn an_employer(db: &SaltDatabase) -> EmployerId {
    create_employer(db, "Employer", monthly_schedule(), "actor")
        .await
        .unwrap()
}

/// An Employment starting on `period()`'s own first day, with every fact
/// `calculate` needs already on record: `CompensationTerms` effective from
/// that same day, a confirmed absence of `PriorEmployment`, and a confirmed
/// absence of unsupported deductions. `period()` is this Employment's first
/// payable period, so no `OpeningBalance` is required (§7.1 branch 1).
async fn a_fully_declared_employment(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    person: &str,
    basic_pay: Money,
) -> EmploymentId {
    let employment_id = create_employment(
        db,
        employer_id,
        &PersonId::new(person),
        period().start(),
        None,
        "actor",
    )
    .await
    .unwrap();
    record_compensation_terms(
        db,
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
        db,
        &employment_id,
        TaxYear::for_period_end(period().end()),
        PriorEmployment::None,
        "actor",
    )
    .await
    .unwrap();
    declare_unsupported_deduction_status(
        db,
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

/// Creates a March Ordinary run and calculates it to `Calculated`.
async fn a_calculated_run(db: &SaltDatabase, employer_id: &EmployerId) -> PayrollRunId {
    let run_id = create_ordinary_payroll_run(db, employer_id, period(), date(2026, 4, 5), "actor")
        .await
        .unwrap();
    let refusals = calculate_payroll_run(db, &run_id, "calculator")
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
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = a_fully_declared_employment(
        &db,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let run_id = a_calculated_run(&db, &employer_id).await;

    finalize_payroll_run(&db, &run_id, "finalizer")
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
    assert_eq!(row.snapshot_schema_version, SNAPSHOT_SCHEMA_VERSION);
    assert_eq!(
        SNAPSHOT_SCHEMA_VERSION, 1,
        "§9: the snapshot ships at version 1"
    );
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
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let run_id = a_calculated_run(&db, &employer_id).await;

    finalize_payroll_run(&db, &run_id, "finalizer")
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
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = a_fully_declared_employment(
        &db,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let march_run_id = a_calculated_run(&db, &employer_id).await;
    finalize_payroll_run(&db, &march_run_id, "finalizer")
        .await
        .unwrap();
    let march = finalized_payroll_row(&pool, &march_run_id, &employment_id)
        .await
        .unwrap();

    let april_run_id =
        create_ordinary_payroll_run(&db, &employer_id, next_period(), date(2026, 5, 5), "actor")
            .await
            .unwrap();
    let refusals = calculate_payroll_run(&db, &april_run_id, "calculator")
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
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let run_id = a_calculated_run(&db, &employer_id).await;
    sqlx::query("DELETE FROM payroll_run WHERE id = $1::uuid")
        .bind(run_id.as_str())
        .execute(&pool)
        .await
        .unwrap();

    let result = finalize_payroll_run(&db, &run_id, "finalizer").await;

    assert_eq!(result, Err(PayrollAppError::PayrollRunNotFound(run_id)));
}

#[sqlx::test]
async fn finalizing_a_draft_run_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, period(), date(2026, 4, 5), "actor")
            .await
            .unwrap();

    let result = finalize_payroll_run(&db, &run_id, "finalizer").await;

    assert_eq!(
        result,
        Err(PayrollAppError::PayrollRunNotCalculated(run_id))
    );
}

#[sqlx::test]
async fn finalizing_an_already_finalized_run_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let run_id = a_calculated_run(&db, &employer_id).await;
    finalize_payroll_run(&db, &run_id, "finalizer")
        .await
        .unwrap();

    let result = finalize_payroll_run(&db, &run_id, "finalizer").await;

    assert_eq!(
        result,
        Err(PayrollAppError::PayrollRunAlreadyFinalized {
            payroll_run_id: run_id,
            finalized_payroll_id: None,
        })
    );
}

/// The immutable history row remains the identity of the run even after a
/// reversal removes it from the liveness index. A retry must not return null
/// or a later Correction's replacement row.
#[sqlx::test]
async fn an_already_finalized_run_names_its_history_after_reversal(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    a_fully_declared_employment(
        &db,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let run_id = a_calculated_run(&db, &employer_id).await;
    let outcome = finalize_payroll_run(&db, &run_id, "finalizer")
        .await
        .unwrap();
    let finalized_payroll_id = outcome.finalized[0].1.clone();

    reverse_finalized_payroll(
        &db,
        &finalized_payroll_id,
        "the payroll was wrong",
        "reverser",
    )
    .await
    .unwrap();

    let result = finalize_payroll_run(&db, &run_id, "finalizer").await;

    assert_eq!(
        result,
        Err(PayrollAppError::PayrollRunAlreadyFinalized {
            payroll_run_id: run_id,
            finalized_payroll_id: Some(finalized_payroll_id),
        })
    );
}

/// Two members each finalize into their own separate `FinalizedPayroll` row
/// (issue #50, §0.28) — there is no single one to name, so a retried
/// finalize gets `None` rather than either of them chosen arbitrarily.
#[sqlx::test]
async fn an_already_finalized_run_with_two_members_names_no_single_finalized_payroll(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    a_fully_declared_employment(
        &db,
        &employer_id,
        "alice",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    a_fully_declared_employment(
        &db,
        &employer_id,
        "bob",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, period(), date(2026, 4, 5), "actor")
            .await
            .unwrap();
    calculate_payroll_run(&db, &run_id, "calculator")
        .await
        .unwrap();
    finalize_payroll_run(&db, &run_id, "finalizer")
        .await
        .unwrap();

    let result = finalize_payroll_run(&db, &run_id, "finalizer").await;

    assert_eq!(
        result,
        Err(PayrollAppError::PayrollRunAlreadyFinalized {
            payroll_run_id: run_id,
            finalized_payroll_id: None,
        })
    );
}

// ---- Three-way finalization equality (§5.2, §14 tests 30-33) ----

/// The tempting simplification is comparing the `PayrollCalculation` alone.
/// This tampers the *stored* calculation directly — the only way to make a
/// recomputed `PayrollCalculation` disagree while its own inputs and rules
/// have not moved, since `calculate` is otherwise deterministic in them.
#[sqlx::test]
async fn finalization_refuses_when_the_recomputed_calculation_differs(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = a_fully_declared_employment(
        &db,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let run_id = a_calculated_run(&db, &employer_id).await;

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

    let result = finalize_payroll_run(&db, &run_id, "finalizer").await;

    assert!(
        matches!(
            result,
            Err(PayrollAppError::FinalizationCalculationMismatch { employment_id: ref id, .. })
                if *id == employment_id
        ),
        "expected a FinalizationCalculationMismatch, got {result:?}"
    );
    // ADR-0010: the refusal names *what changed inside* the value that
    // differed, not only which of the three it was.
    let message = result.unwrap_err().to_string();
    assert!(message.contains("net_pay"), "{message}");
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
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = a_fully_declared_employment(
        &db,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let run_id = a_calculated_run(&db, &employer_id).await;
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

    let result = finalize_payroll_run(&db, &run_id, "finalizer").await;

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
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = a_fully_declared_employment(
        &db,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let run_id = a_calculated_run(&db, &employer_id).await;

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

    let result = finalize_payroll_run(&db, &run_id, "finalizer").await;

    assert!(
        matches!(
            result,
            Err(PayrollAppError::FinalizationRulesMismatch { employment_id: ref id, .. })
                if *id == employment_id
        ),
        "expected a FinalizationRulesMismatch, got {result:?}"
    );
    let message = result.as_ref().unwrap_err().to_string();
    assert!(message.contains("ssc_ruleset.id"), "{message}");
    assert!(message.contains("ssc-2025-03-tampered"), "{message}");

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
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let good = a_fully_declared_employment(
        &db,
        &employer_id,
        "person-good",
        Money::from_cents(900000).unwrap(),
    )
    .await;
    let bad = a_fully_declared_employment(
        &db,
        &employer_id,
        "person-bad",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    assert!(
        good.as_str() < bad.as_str(),
        "UUIDv7 ids order by creation, so the tampered member must be rebuilt second"
    );
    let run_id = a_calculated_run(&db, &employer_id).await;

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

    let result = finalize_payroll_run(&db, &run_id, "finalizer").await;

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
    // The rows, the liveness, the audit entry and the status change are one
    // transaction (§5.3, §10), so a refusal leaves none of the four behind —
    // not a `PayrollFinalized` entry for a run that never finalized, and not
    // a status that says it did.
    assert_eq!(
        action_log_count(&pool, "payroll_finalized", run_id.as_str()).await,
        0
    );
    assert_eq!(run_status(&pool, &run_id).await, "calculated");
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
// spawned this way, `calculate_payroll_run` included. `tokio::join!` is the
// way round it: it polls both futures in place, so no `Send` bound is ever
// required, while each still draws its own connection from the pool — two
// real PostgreSQL transactions racing for one row.
//
// So the test proves the guarantee in two steps. Raw SQL, on two real
// connections with a controlled interleaving, proves the `FOR UPDATE` lock
// genuinely blocks a second transaction rather than merely being present in
// the SQL. Then two real `finalize_payroll_run` calls, simultaneously in
// flight, prove that exactly one of them succeeds and that the loser's own
// refusal — read from the status the winner committed, not from an
// application-side check made before the race — is what the blocked side
// sees.
#[sqlx::test]
async fn two_finalizations_of_the_same_run_end_with_exactly_one_success(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id = a_fully_declared_employment(
        &db,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let run_id = a_calculated_run(&db, &employer_id).await;

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

    // Phase 2: two *real* `finalize_payroll_run` calls, genuinely in flight
    // at once. `tokio::join!` polls both futures in place rather than
    // spawning them, so neither needs the `Send` bound the note above
    // explains this crate's async functions cannot prove — and both still
    // take their own connection from the pool, so the interleaving is two
    // real PostgreSQL transactions racing for one row, not two turns of the
    // same one. Which of them wins the `FOR UPDATE` is PostgreSQL's to
    // decide and this test does not care; that exactly one does is the
    // whole invariant (§5.4).
    let (first, second) = tokio::join!(
        finalize_payroll_run(&db, &run_id, "finalizer-a"),
        finalize_payroll_run(&db, &run_id, "finalizer-b"),
    );

    let (winner, loser) = match (&first, &second) {
        (Ok(_), Err(_)) => (&first, &second),
        (Err(_), Ok(_)) => (&second, &first),
        _ => panic!("exactly one finalization must succeed, got {first:?} and {second:?}"),
    };
    assert_eq!(
        winner.as_ref().ok().map(|outcome| outcome.finalized.len()),
        Some(1),
        "the winner finalizes the run's one member"
    );
    // The loser is refused by re-reading the status the winner committed —
    // the lock releasing is what lets it read at all — not by an
    // application-side check made before the race. It also gets back the
    // one `FinalizedPayrollId` the winner actually minted, read from
    // `live_finalized_payroll` rather than assumed: the run has exactly one
    // member, so that id is unambiguous.
    let finalized_payroll_id = winner
        .as_ref()
        .ok()
        .and_then(|outcome| outcome.finalized.first())
        .map(|(_, id)| id.clone());
    assert_eq!(
        *loser,
        Err(PayrollAppError::PayrollRunAlreadyFinalized {
            payroll_run_id: run_id.clone(),
            finalized_payroll_id,
        })
    );
    assert_eq!(run_status(&pool, &run_id).await, "finalized");

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

/// §5.1 and user story 30: a member whose rebuild refuses outright — rather
/// than merely differing — blocks the whole run, and the refusal says *which*
/// Employment to go and fix.
///
/// `PayrollError::PriorEmploymentUnknown` carries no `EmploymentId` of its
/// own, which is exactly why finalization must name one: without it an
/// Employer with ten members is told a fact about the run and nothing about
/// where to look. The declaration is withdrawn directly, since the public API
/// has no "undeclare" use case — that is the point, this is a fact moving
/// underneath an approved run.
#[sqlx::test]
async fn a_members_rebuild_refusal_names_the_employment_and_finalizes_nobody(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let good = a_fully_declared_employment(
        &db,
        &employer_id,
        "person-good",
        Money::from_cents(900000).unwrap(),
    )
    .await;
    let bad = a_fully_declared_employment(
        &db,
        &employer_id,
        "person-bad",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    assert!(
        good.as_str() < bad.as_str(),
        "UUIDv7 ids order by creation, so the refusing member must be rebuilt second"
    );
    let run_id = a_calculated_run(&db, &employer_id).await;

    sqlx::query("DELETE FROM prior_employment_declaration WHERE employment_id = $1")
        .bind(bad.as_str())
        .execute(&pool)
        .await
        .unwrap();

    let result = finalize_payroll_run(&db, &run_id, "finalizer").await;

    assert_eq!(
        result,
        Err(PayrollAppError::FinalizationRebuildRefused {
            employment_id: bad.clone(),
            refusal: Box::new(PayrollAppError::Payroll(
                payroll::PayrollError::PriorEmploymentUnknown
            )),
        })
    );
    let message = result.unwrap_err().to_string();
    assert!(message.contains(bad.as_str()), "{message}");

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM finalized_payroll")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "the member that rebuilt cleanly must not finalize either"
    );
    assert_eq!(run_status(&pool, &run_id).await, "calculated");
}
