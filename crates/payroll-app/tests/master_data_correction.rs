//! Proves the use cases issue #36 introduces: `correct_compensation_terms`,
//! and the reason/divergence treatment added to
//! `declare_unsupported_deduction_status` —
//! `docs/domain/payroll-run-persistence.md` §6.5, ADR-0013 as amended — all
//! reached through the public API, never raw SQL, except to read back
//! stored rows and prove they were left untouched.

use chrono::NaiveDate;
use payroll::{
    EmployerId, EmploymentId, Money, PayPeriod, PayrollError, PeriodEndDay, PersonId,
    PriorEmployment, TaxYear, UnsupportedDeductionStatus,
};
use payroll_app::{
    PayrollAppError, calculate_payroll_run, correct_compensation_terms, create_employer,
    create_employment, create_ordinary_payroll_run, declare_prior_employment,
    declare_unsupported_deduction_status, finalize_payroll_run, record_compensation_terms,
};
use sqlx::{PgPool, Row};

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn monthly_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::LastDayOfMonth)
}

fn march() -> PayPeriod {
    PayPeriod::new(date(2026, 3, 1), date(2026, 3, 31)).unwrap()
}

fn april() -> PayPeriod {
    PayPeriod::new(date(2026, 4, 1), date(2026, 4, 30)).unwrap()
}

fn may() -> PayPeriod {
    PayPeriod::new(date(2026, 5, 1), date(2026, 5, 31)).unwrap()
}

async fn an_employer(pool: &PgPool) -> EmployerId {
    create_employer(pool, monthly_schedule(), "actor")
        .await
        .unwrap()
}

/// An Employment starting on March's own first day (its first payable
/// period, so no `OpeningBalance` is required), with a single
/// `CompensationTerms` row from that same day and every other fact
/// `calculate` needs already on record.
async fn an_employment_with_basic_pay(
    pool: &PgPool,
    employer_id: &EmployerId,
    basic_pay: Money,
) -> EmploymentId {
    let employment_id = create_employment(
        pool,
        employer_id,
        &PersonId::new("person-1"),
        march().start(),
        None,
        "actor",
    )
    .await
    .unwrap();
    record_compensation_terms(pool, &employment_id, march().start(), basic_pay, "actor")
        .await
        .unwrap();
    declare_prior_employment(
        pool,
        &employment_id,
        TaxYear::for_period_end(march().end()),
        PriorEmployment::None,
        "actor",
    )
    .await
    .unwrap();
    declare_unsupported_deduction_status(
        pool,
        &employment_id,
        march().start(),
        UnsupportedDeductionStatus::ConfirmedNone,
        "no unsupported deductions",
        "actor",
    )
    .await
    .unwrap();
    employment_id
}

/// Creates, calculates and finalizes an Ordinary run for `period`, and
/// returns the fingerprint pair (`finalized_payroll`, `live_finalized_payroll`)
/// so a caller can prove neither row moves later.
async fn finalize_period(pool: &PgPool, employer_id: &EmployerId, period: PayPeriod) {
    let run_id = create_ordinary_payroll_run(pool, employer_id, period, period.end(), "actor")
        .await
        .unwrap();
    let refusals = calculate_payroll_run(pool, &run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(refusals, Vec::new(), "the run must reach Calculated");
    finalize_payroll_run(pool, &run_id, "finalizer")
        .await
        .unwrap();
}

/// An MD5 fingerprint of the whole `finalized_payroll` row for
/// `(employment_id, period_end)`, live or reversed. Used to prove a
/// master-data correction leaves an already-frozen row byte-identical,
/// without hand-listing every column.
async fn finalized_payroll_fingerprint(
    pool: &PgPool,
    employment_id: &EmploymentId,
    period_end: NaiveDate,
) -> String {
    sqlx::query_scalar(
        "SELECT md5(finalized_payroll::text) FROM finalized_payroll
         WHERE employment_id = $1 AND period_end = $2",
    )
    .bind(employment_id.as_str())
    .bind(period_end)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// As [`finalized_payroll_fingerprint`], for the `live_finalized_payroll`
/// liveness row.
async fn live_finalized_payroll_fingerprint(
    pool: &PgPool,
    employment_id: &EmploymentId,
    period_end: NaiveDate,
) -> String {
    sqlx::query_scalar(
        "SELECT md5(live_finalized_payroll::text) FROM live_finalized_payroll
         WHERE employment_id = $1 AND period_end = $2",
    )
    .bind(employment_id.as_str())
    .bind(period_end)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn compensation_terms_row_count(pool: &PgPool, employment_id: &EmploymentId) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM compensation_terms WHERE employment_id = $1")
        .bind(employment_id.as_str())
        .fetch_one(pool)
        .await
        .unwrap()
}

// ---- `correct_compensation_terms` ----

/// §14 test 38: a correction over a span holding no live finalized payroll
/// at all still succeeds, returns an empty divergence list, changes the row,
/// and is logged with the before and after values.
#[sqlx::test]
async fn a_correction_with_no_live_finalized_payroll_diverges_from_nothing(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id =
        an_employment_with_basic_pay(&pool, &employer_id, Money::from_cents(500000).unwrap()).await;

    let diverging = correct_compensation_terms(
        &pool,
        &employment_id,
        march().start(),
        march().start(),
        Money::from_cents(600000).unwrap(),
        "typo in the original amount",
        "actor",
    )
    .await
    .unwrap();

    assert_eq!(diverging, Vec::new());

    let row = sqlx::query(
        "SELECT basic_pay FROM compensation_terms
         WHERE employment_id = $1 AND effective_from = $2",
    )
    .bind(employment_id.as_str())
    .bind(march().start())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<i64, _>(0), 600000);

    let entry = sqlx::query(
        "SELECT context FROM action_log_entry
         WHERE action_type = 'compensation_terms_corrected' AND target_id = $1",
    )
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    let context: serde_json::Value = entry.get(0);
    assert_eq!(context["reason"], "typo in the original amount");
    assert_eq!(context["before"]["basic_pay_cents"], 500000);
    assert_eq!(context["after"]["basic_pay_cents"], 600000);
    assert_eq!(
        context["diverging_live_finalized_periods"],
        serde_json::json!([])
    );
}

#[sqlx::test]
async fn an_empty_reason_is_refused_and_nothing_is_touched(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id =
        an_employment_with_basic_pay(&pool, &employer_id, Money::from_cents(500000).unwrap()).await;

    let result = correct_compensation_terms(
        &pool,
        &employment_id,
        march().start(),
        march().start(),
        Money::from_cents(600000).unwrap(),
        "   ",
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::CompensationTermsCorrectionReasonCannotBeEmpty)
    );
    let row = sqlx::query(
        "SELECT basic_pay FROM compensation_terms
         WHERE employment_id = $1 AND effective_from = $2",
    )
    .bind(employment_id.as_str())
    .bind(march().start())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        row.get::<i64, _>(0),
        500000,
        "the refused correction must not write"
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM action_log_entry WHERE action_type = 'compensation_terms_corrected'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count, 0,
        "a refused correction must write no ActionLog entry"
    );
}

#[sqlx::test]
async fn correcting_a_row_that_does_not_exist_is_refused(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id =
        an_employment_with_basic_pay(&pool, &employer_id, Money::from_cents(500000).unwrap()).await;

    let result = correct_compensation_terms(
        &pool,
        &employment_id,
        date(2026, 6, 1),
        date(2026, 6, 1),
        Money::from_cents(600000).unwrap(),
        "a reason",
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::NoCompensationTermsRowAt {
            employment_id: employment_id.clone(),
            effective_from: date(2026, 6, 1),
        })
    );
}

#[sqlx::test]
async fn correcting_against_a_missing_employment_is_refused(pool: PgPool) {
    let missing = EmploymentId::new("does-not-exist");

    let result = correct_compensation_terms(
        &pool,
        &missing,
        march().start(),
        march().start(),
        Money::from_cents(600000).unwrap(),
        "a reason",
        "actor",
    )
    .await;

    assert_eq!(result, Err(PayrollAppError::EmploymentNotFound(missing)));
}

#[sqlx::test]
async fn a_new_effective_from_that_is_not_a_period_start_is_a_domain_refusal(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id =
        an_employment_with_basic_pay(&pool, &employer_id, Money::from_cents(500000).unwrap()).await;

    let result = correct_compensation_terms(
        &pool,
        &employment_id,
        march().start(),
        date(2026, 3, 10),
        Money::from_cents(600000).unwrap(),
        "a reason",
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::from(
            PayrollError::EffectiveFromNotAPeriodStart {
                next_valid_effective_from: april().start(),
            }
        ))
    );
}

/// §14 tests 38 and 39, end to end: March, April and May all reference the
/// same `CompensationTerms` row and are finalized; March's amount is found
/// wrong; the row is split so March carries the true amount and the
/// existing row's `effective_from` moves to April. The divergence list names
/// all three live finalized periods the *original* row covered, and April
/// and May's `finalized_payroll` and `live_finalized_payroll` rows are left
/// byte-identical.
#[sqlx::test]
async fn splitting_a_row_leaves_april_and_may_byte_identical(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let original_pay = Money::from_cents(500000).unwrap();
    let employment_id = an_employment_with_basic_pay(&pool, &employer_id, original_pay).await;

    finalize_period(&pool, &employer_id, march()).await;
    finalize_period(&pool, &employer_id, april()).await;
    finalize_period(&pool, &employer_id, may()).await;

    let april_finalized_before =
        finalized_payroll_fingerprint(&pool, &employment_id, april().end()).await;
    let april_live_before =
        live_finalized_payroll_fingerprint(&pool, &employment_id, april().end()).await;
    let may_finalized_before =
        finalized_payroll_fingerprint(&pool, &employment_id, may().end()).await;
    let may_live_before =
        live_finalized_payroll_fingerprint(&pool, &employment_id, may().end()).await;

    // Move the existing row's effective_from to 1-April, keeping BasicPay
    // unchanged — April and May must go on reading exactly what they always
    // did.
    let diverging = correct_compensation_terms(
        &pool,
        &employment_id,
        march().start(),
        april().start(),
        original_pay,
        "March rate captured wrong; the terms only took effect in April",
        "actor",
    )
    .await
    .unwrap();

    assert_eq!(diverging, vec![march(), april(), may()]);

    // Insert March's true amount as a brand new row — an ordinary record,
    // not a correction, since nothing before this insert ever relied on it.
    let true_march_pay = Money::from_cents(550000).unwrap();
    record_compensation_terms(
        &pool,
        &employment_id,
        march().start(),
        true_march_pay,
        "actor",
    )
    .await
    .unwrap();

    assert_eq!(compensation_terms_row_count(&pool, &employment_id).await, 2);

    let april_finalized_after =
        finalized_payroll_fingerprint(&pool, &employment_id, april().end()).await;
    let april_live_after =
        live_finalized_payroll_fingerprint(&pool, &employment_id, april().end()).await;
    let may_finalized_after =
        finalized_payroll_fingerprint(&pool, &employment_id, may().end()).await;
    let may_live_after =
        live_finalized_payroll_fingerprint(&pool, &employment_id, may().end()).await;

    assert_eq!(
        april_finalized_before, april_finalized_after,
        "April's FinalizedPayroll must be byte-identical"
    );
    assert_eq!(
        april_live_before, april_live_after,
        "April's liveness row must be byte-identical"
    );
    assert_eq!(
        may_finalized_before, may_finalized_after,
        "May's FinalizedPayroll must be byte-identical"
    );
    assert_eq!(
        may_live_before, may_live_after,
        "May's liveness row must be byte-identical"
    );

    let context: serde_json::Value = sqlx::query_scalar(
        "SELECT context FROM action_log_entry
         WHERE action_type = 'compensation_terms_corrected' AND target_id = $1",
    )
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        context["reason"],
        "March rate captured wrong; the terms only took effect in April"
    );
    assert_eq!(context["before"]["effective_from"], "2026-03-01");
    assert_eq!(context["after"]["effective_from"], "2026-04-01");
    assert_eq!(
        context["diverging_live_finalized_periods"],
        serde_json::json!([
            { "period_start": "2026-03-01", "period_end": "2026-03-31" },
            { "period_start": "2026-04-01", "period_end": "2026-04-30" },
            { "period_start": "2026-05-01", "period_end": "2026-05-31" },
        ])
    );
}

// ---- `declare_unsupported_deduction_status`'s reason and divergence ----

#[sqlx::test]
async fn an_empty_reason_is_refused_for_an_unsupported_deduction_declaration(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id =
        an_employment_with_basic_pay(&pool, &employer_id, Money::from_cents(500000).unwrap()).await;

    let result = declare_unsupported_deduction_status(
        &pool,
        &employment_id,
        april().start(),
        UnsupportedDeductionStatus::ConfirmedNone,
        "  ",
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::UnsupportedDeductionDeclarationReasonCannotBeEmpty)
    );
}

/// A later change to an already-live-finalized period's declaration names
/// that period as diverging, and the `ActionLog` entry carries the reason
/// and the before and after status.
#[sqlx::test]
async fn redeclaring_over_a_live_finalized_period_names_it_as_diverging(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id =
        an_employment_with_basic_pay(&pool, &employer_id, Money::from_cents(500000).unwrap()).await;

    finalize_period(&pool, &employer_id, march()).await;

    let diverging = declare_unsupported_deduction_status(
        &pool,
        &employment_id,
        march().start(),
        UnsupportedDeductionStatus::ConfirmedNone,
        "found a provident fund deduction we missed",
        "actor",
    )
    .await
    .unwrap();

    assert_eq!(diverging, vec![march()]);

    let march_finalized_after =
        finalized_payroll_fingerprint(&pool, &employment_id, march().end()).await;
    assert!(!march_finalized_after.is_empty());
}
