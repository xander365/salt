//! Proves `reverse_finalized_payroll` — issue #32,
//! `docs/domain/payroll-run-persistence.md` §6.1 and §12 — reached through
//! the public API. Every fixture finalizes a real `FinalizedPayroll` through
//! `finalize_payroll_run` first, exactly as `tests/finalize_payroll_run.rs`
//! does, since a `FinalizedPayrollId` is only ever minted there.

use chrono::NaiveDate;
use payroll::{
    EmployerId, EmploymentId, Money, PayPeriod, PeriodEndDay, PersonId, PriorEmployment, TaxYear,
    UnsupportedDeductionStatus,
};
use payroll_app::{
    FinalizedPayrollId, PayrollAppError, build_year_to_date_context, calculate_payroll_run,
    create_employer, create_employment, create_ordinary_payroll_run, declare_prior_employment,
    declare_unsupported_deduction_status, finalize_payroll_run, record_compensation_terms,
    reverse_finalized_payroll,
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

/// The `PayPeriod` immediately after `period()`, under the same schedule —
/// used by the year-to-date test.
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

/// Creates, calculates and finalizes a March Ordinary run for one fully
/// declared Employment, and returns the `FinalizedPayrollId` it produced.
async fn a_finalized_payroll(
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

struct FinalizedPayrollSnapshot {
    input_json: serde_json::Value,
    taxable_remuneration_cents: i64,
    finalized_by: String,
}

async fn finalized_payroll_snapshot(
    pool: &PgPool,
    finalized_payroll_id: &FinalizedPayrollId,
) -> FinalizedPayrollSnapshot {
    let row = sqlx::query(
        "SELECT payroll_input_json, taxable_remuneration, finalized_by
         FROM finalized_payroll WHERE id = $1::uuid",
    )
    .bind(finalized_payroll_id.as_str())
    .fetch_one(pool)
    .await
    .unwrap();
    FinalizedPayrollSnapshot {
        input_json: row.get(0),
        taxable_remuneration_cents: row.get(1),
        finalized_by: row.get(2),
    }
}

struct ReversalRow {
    reversed_by: String,
    reason: String,
    reversed_at: chrono::DateTime<chrono::Utc>,
}

async fn reversal_row(
    pool: &PgPool,
    finalized_payroll_id: &FinalizedPayrollId,
) -> Option<ReversalRow> {
    sqlx::query(
        "SELECT reversed_by, reason, reversed_at FROM reversal
         WHERE finalized_payroll_id = $1::uuid",
    )
    .bind(finalized_payroll_id.as_str())
    .fetch_optional(pool)
    .await
    .unwrap()
    .map(|row| ReversalRow {
        reversed_by: row.get(0),
        reason: row.get(1),
        reversed_at: row.get(2),
    })
}

async fn reversal_count(pool: &PgPool, finalized_payroll_id: &FinalizedPayrollId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM reversal WHERE finalized_payroll_id = $1::uuid")
        .bind(finalized_payroll_id.as_str())
        .fetch_one(pool)
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

// ---- The tracer bullet: reverses, leaves the original untouched ----

#[sqlx::test]
async fn reversing_a_finalized_payroll_records_the_reason_actor_and_deletes_liveness(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let finalized_payroll_id = a_finalized_payroll(&pool, &employer_id, &employment_id).await;
    let before = finalized_payroll_snapshot(&pool, &finalized_payroll_id).await;
    assert!(
        live_finalized_payroll_id(&pool, &employment_id, period().end())
            .await
            .is_some(),
        "the finalized period must start out live"
    );

    reverse_finalized_payroll(
        &pool,
        &finalized_payroll_id,
        "March rate captured wrong",
        "hr",
    )
    .await
    .unwrap();

    // The liveness row is gone.
    assert!(
        live_finalized_payroll_id(&pool, &employment_id, period().end())
            .await
            .is_none(),
        "reversal must delete the liveness row"
    );

    // The Reversal itself names the reason and the actor.
    let reversal = reversal_row(&pool, &finalized_payroll_id)
        .await
        .expect("a Reversal row must exist");
    assert_eq!(reversal.reversed_by, "hr");
    assert_eq!(reversal.reason, "March rate captured wrong");
    // And the time. `reversed_at` defaults to `now()` in the database rather
    // than to a clock the application passes in, so the only honest
    // assertion is that it sits around this test's own wall clock.
    let elapsed = chrono::Utc::now() - reversal.reversed_at;
    assert!(
        elapsed >= chrono::TimeDelta::zero() && elapsed < chrono::TimeDelta::minutes(1),
        "the Reversal must record when it happened, got {}",
        reversal.reversed_at
    );

    // The original FinalizedPayroll row is completely untouched.
    let after = finalized_payroll_snapshot(&pool, &finalized_payroll_id).await;
    assert_eq!(after.input_json, before.input_json);
    assert_eq!(
        after.taxable_remuneration_cents,
        before.taxable_remuneration_cents
    );
    assert_eq!(after.finalized_by, before.finalized_by);

    // The Reversal and its ActionLog entry commit together (§10).
    assert_eq!(
        action_log_count(
            &pool,
            "finalized_payroll_reversed",
            finalized_payroll_id.as_str()
        )
        .await,
        1
    );
}

// ---- Reason ----

#[sqlx::test]
async fn reversing_with_a_blank_reason_is_refused_and_writes_nothing(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let finalized_payroll_id = a_finalized_payroll(&pool, &employer_id, &employment_id).await;

    let result = reverse_finalized_payroll(&pool, &finalized_payroll_id, "   ", "hr").await;

    assert_eq!(result, Err(PayrollAppError::ReversalReasonCannotBeEmpty));
    assert_eq!(reversal_count(&pool, &finalized_payroll_id).await, 0);
    assert!(
        live_finalized_payroll_id(&pool, &employment_id, period().end())
            .await
            .is_some(),
        "a refused reversal must leave liveness untouched"
    );
    assert_eq!(
        action_log_count(
            &pool,
            "finalized_payroll_reversed",
            finalized_payroll_id.as_str()
        )
        .await,
        0
    );
}

// ---- Lifecycle refusals ----

#[sqlx::test]
async fn reversing_a_missing_finalized_payroll_is_refused(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let finalized_payroll_id = a_finalized_payroll(&pool, &employer_id, &employment_id).await;
    sqlx::query("DELETE FROM live_finalized_payroll WHERE finalized_payroll_id = $1::uuid")
        .bind(finalized_payroll_id.as_str())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM finalized_payroll WHERE id = $1::uuid")
        .bind(finalized_payroll_id.as_str())
        .execute(&pool)
        .await
        .unwrap();

    let result = reverse_finalized_payroll(&pool, &finalized_payroll_id, "a reason", "hr").await;

    assert_eq!(
        result,
        Err(PayrollAppError::FinalizedPayrollNotFound(
            finalized_payroll_id
        ))
    );
}

/// §6.1: a `FinalizedPayroll` can be reversed only once, and the failed
/// second attempt leaves the audit trail exactly as the first left it — no
/// duplicate `Reversal`, no duplicate `ActionLog` entry.
#[sqlx::test]
async fn reversing_an_already_reversed_finalized_payroll_is_refused(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let finalized_payroll_id = a_finalized_payroll(&pool, &employer_id, &employment_id).await;
    reverse_finalized_payroll(&pool, &finalized_payroll_id, "first reason", "hr")
        .await
        .unwrap();

    let result =
        reverse_finalized_payroll(&pool, &finalized_payroll_id, "second reason", "hr-2").await;

    assert_eq!(
        result,
        Err(PayrollAppError::FinalizedPayrollAlreadyReversed(
            finalized_payroll_id.clone()
        ))
    );
    assert_eq!(reversal_count(&pool, &finalized_payroll_id).await, 1);
    let reversal = reversal_row(&pool, &finalized_payroll_id).await.unwrap();
    assert_eq!(
        reversal.reason, "first reason",
        "the refused second attempt must not overwrite the first Reversal"
    );
    assert_eq!(
        action_log_count(
            &pool,
            "finalized_payroll_reversed",
            finalized_payroll_id.as_str()
        )
        .await,
        1,
        "a refused second reversal writes no second audit entry"
    );
}

/// §11: two genuinely concurrent reversals of one `FinalizedPayroll` end
/// with exactly one Reversal, and the loser reads the *same* named refusal a
/// repeat attempt reads. The `UNIQUE (finalized_payroll_id)` constraint from
/// migration 0011 is the whole mechanism — an application-side existence
/// check cannot see the winner's uncommitted row, which is why the refusal is
/// read back out of the constraint violation rather than decided before it.
#[sqlx::test]
async fn two_reversals_of_the_same_finalized_payroll_end_with_exactly_one_success(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let finalized_payroll_id = a_finalized_payroll(&pool, &employer_id, &employment_id).await;

    // `tokio::join!` polls both futures in place rather than spawning them,
    // so neither needs the `Send` bound this crate's async functions cannot
    // prove — and each still takes its own connection from the pool, so this
    // is two real PostgreSQL transactions racing for one row. Which one wins
    // is PostgreSQL's to decide and this test does not care; that exactly one
    // does is the invariant.
    let (first, second) = tokio::join!(
        reverse_finalized_payroll(&pool, &finalized_payroll_id, "reason-a", "hr-a"),
        reverse_finalized_payroll(&pool, &finalized_payroll_id, "reason-b", "hr-b"),
    );

    let loser = match (&first, &second) {
        (Ok(()), Err(_)) => &second,
        (Err(_), Ok(())) => &first,
        _ => panic!("exactly one reversal must succeed, got {first:?} and {second:?}"),
    };
    assert_eq!(
        *loser,
        Err(PayrollAppError::FinalizedPayrollAlreadyReversed(
            finalized_payroll_id.clone()
        )),
        "the loser of a race must read the same refusal a repeat attempt reads, \
         not a raw constraint violation"
    );

    assert_eq!(reversal_count(&pool, &finalized_payroll_id).await, 1);
    assert_eq!(
        action_log_count(
            &pool,
            "finalized_payroll_reversed",
            finalized_payroll_id.as_str()
        )
        .await,
        1,
        "the loser's whole transaction rolls back, audit entry included"
    );
    assert!(
        live_finalized_payroll_id(&pool, &employment_id, period().end())
            .await
            .is_none(),
        "the winner still deleted the liveness row"
    );
}

// ---- Year-to-date ----

/// §6.3: a bare reversal is arithmetically complete on its own — a
/// `YearToDateContext` built afterwards for a later period drops the
/// reversed period immediately, with no replacement required.
#[sqlx::test]
async fn a_year_to_date_context_built_after_reversal_excludes_the_reversed_period(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        Money::from_cents(1500000).unwrap(),
    )
    .await;
    let finalized_payroll_id = a_finalized_payroll(&pool, &employer_id, &employment_id).await;

    // Before reversal: April's year-to-date carries March's figures.
    let before = build_year_to_date_context(&pool, &employment_id, next_period().end())
        .await
        .unwrap();
    assert!(before.prior_taxable_remuneration() > Money::ZERO);

    reverse_finalized_payroll(&pool, &finalized_payroll_id, "March was wrong", "hr")
        .await
        .unwrap();

    let after = build_year_to_date_context(&pool, &employment_id, next_period().end())
        .await
        .unwrap();
    assert_eq!(after.prior_taxable_remuneration(), Money::ZERO);
    assert_eq!(after.prior_paye(), Money::ZERO);
}
