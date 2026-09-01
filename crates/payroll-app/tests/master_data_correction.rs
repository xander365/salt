//! Proves every §6.5 write that can make master data disagree with paid
//! history: `correct_compensation_terms` and the reason/divergence treatment
//! added to `declare_unsupported_deduction_status` (issue #36), and the same
//! treatment on the one `record_compensation_terms` path that diverges
//! (issue #37) — `docs/domain/payroll-run-persistence.md` §6.5, ADR-0013 as
//! amended. All reached through the public API, never raw SQL, except to
//! read back stored rows and prove they were left untouched.

use chrono::NaiveDate;
use payroll::{
    EmployerId, EmploymentId, Money, PayPeriod, PayrollError, PeriodEndDay, PersonId,
    PriorEmployment, TaxYear, UnsupportedDeductionKind, UnsupportedDeductionKinds,
    UnsupportedDeductionStatus,
};
use payroll_app::{
    FinalizedPayrollId, PayrollAppError, SaltDatabase, add_employment_to_correction_run,
    build_year_to_date_context, calculate_payroll_run, correct_compensation_terms,
    create_correction_run, create_employer, create_employment, create_ordinary_payroll_run,
    declare_prior_employment, declare_unsupported_deduction_status, finalize_payroll_run,
    get_unsupported_deduction_status, record_compensation_terms, reverse_finalized_payroll,
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

fn june() -> PayPeriod {
    PayPeriod::new(date(2026, 6, 1), date(2026, 6, 30)).unwrap()
}

async fn an_employer(db: &SaltDatabase) -> EmployerId {
    create_employer(db, monthly_schedule(), "actor")
        .await
        .unwrap()
}

/// An Employment starting on March's own first day (its first payable
/// period, so no `OpeningBalance` is required), with a single
/// `CompensationTerms` row from that same day and every other fact
/// `calculate` needs already on record.
async fn an_employment_with_basic_pay(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    basic_pay: Money,
) -> EmploymentId {
    let employment_id = create_employment(
        db,
        employer_id,
        &PersonId::new("person-1"),
        march().start(),
        None,
        "actor",
    )
    .await
    .unwrap();
    record_compensation_terms(
        db,
        &employment_id,
        march().start(),
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
        TaxYear::for_period_end(march().end()),
        PriorEmployment::None,
        "actor",
    )
    .await
    .unwrap();
    declare_unsupported_deduction_status(
        db,
        &employment_id,
        march().start(),
        UnsupportedDeductionStatus::ConfirmedNone,
        &[],
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
async fn finalize_period(db: &SaltDatabase, employer_id: &EmployerId, period: PayPeriod) {
    let run_id = create_ordinary_payroll_run(db, employer_id, period, period.end(), "actor")
        .await
        .unwrap();
    let refusals = calculate_payroll_run(db, &run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(refusals, Vec::new(), "the run must reach Calculated");
    finalize_payroll_run(db, &run_id, "finalizer")
        .await
        .unwrap();
}

/// As [`finalize_period`], but hands back the `FinalizedPayrollId` written
/// for `employment_id`, for a test that has to reverse that exact record.
async fn finalize_period_returning_id(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    employment_id: &EmploymentId,
    period: PayPeriod,
) -> FinalizedPayrollId {
    let run_id = create_ordinary_payroll_run(db, employer_id, period, period.end(), "actor")
        .await
        .unwrap();
    let refusals = calculate_payroll_run(db, &run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(refusals, Vec::new(), "the run must reach Calculated");
    finalize_payroll_run(db, &run_id, "finalizer")
        .await
        .unwrap()
        .finalized
        .into_iter()
        .find(|(id, _)| id == employment_id)
        .expect("the period must have finalized for this Employment")
        .1
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

/// The `basic_pay` of the `CompensationTerms` row at `effective_from`, in
/// cents. A refused correction must leave it exactly as it was.
async fn basic_pay_cents_at(
    pool: &PgPool,
    employment_id: &EmploymentId,
    effective_from: NaiveDate,
) -> i64 {
    sqlx::query_scalar(
        "SELECT basic_pay FROM compensation_terms
         WHERE employment_id = $1 AND effective_from = $2",
    )
    .bind(employment_id.as_str())
    .bind(effective_from)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// How many `CompensationTermsCorrected` entries this Employment has. A
/// refused correction writes none, so the ActionLog can never record an
/// acknowledgement of a correction that never happened.
async fn correction_entry_count(pool: &PgPool, employment_id: &EmploymentId) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM action_log_entry
         WHERE action_type = 'compensation_terms_corrected' AND target_id = $1",
    )
    .bind(employment_id.as_str())
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Every frozen figure this Employment has, as `(period_end,
/// taxable_remuneration, paye)` — the numeric columns year-to-date actually
/// sums (ADR-0012), read in period order.
async fn frozen_figures(pool: &PgPool, employment_id: &EmploymentId) -> Vec<(NaiveDate, i64, i64)> {
    sqlx::query_as(
        "SELECT period_end, taxable_remuneration, paye FROM finalized_payroll
         WHERE employment_id = $1 ORDER BY period_end",
    )
    .bind(employment_id.as_str())
    .fetch_all(pool)
    .await
    .unwrap()
}

/// Every `CompensationTermsCorrected` context this Employment has, in the
/// order the acts happened. A split writes two — the move, then the insert
/// over the span the move freed — and each must say what it actually did.
async fn correction_contexts(
    pool: &PgPool,
    employment_id: &EmploymentId,
) -> Vec<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT context FROM action_log_entry
         WHERE action_type = 'compensation_terms_corrected' AND target_id = $1
         ORDER BY occurred_at",
    )
    .bind(employment_id.as_str())
    .fetch_all(pool)
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
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id =
        an_employment_with_basic_pay(&db, &employer_id, Money::from_cents(500000).unwrap()).await;

    let diverging = correct_compensation_terms(
        &db,
        &employment_id,
        march().start(),
        march().start(),
        Money::from_cents(600000).unwrap(),
        &[],
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
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id =
        an_employment_with_basic_pay(&db, &employer_id, Money::from_cents(500000).unwrap()).await;

    let result = correct_compensation_terms(
        &db,
        &employment_id,
        march().start(),
        march().start(),
        Money::from_cents(600000).unwrap(),
        &[],
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
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id =
        an_employment_with_basic_pay(&db, &employer_id, Money::from_cents(500000).unwrap()).await;

    let result = correct_compensation_terms(
        &db,
        &employment_id,
        date(2026, 6, 1),
        date(2026, 6, 1),
        Money::from_cents(600000).unwrap(),
        &[],
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
    let db = SaltDatabase::from_pool(pool.clone());
    let missing = EmploymentId::new("does-not-exist");

    let result = correct_compensation_terms(
        &db,
        &missing,
        march().start(),
        march().start(),
        Money::from_cents(600000).unwrap(),
        &[],
        "a reason",
        "actor",
    )
    .await;

    assert_eq!(result, Err(PayrollAppError::EmploymentNotFound(missing)));
}

#[sqlx::test]
async fn a_new_effective_from_that_is_not_a_period_start_is_a_domain_refusal(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id =
        an_employment_with_basic_pay(&db, &employer_id, Money::from_cents(500000).unwrap()).await;

    let result = correct_compensation_terms(
        &db,
        &employment_id,
        march().start(),
        date(2026, 3, 10),
        Money::from_cents(600000).unwrap(),
        &[],
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
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let original_pay = Money::from_cents(500000).unwrap();
    let employment_id = an_employment_with_basic_pay(&db, &employer_id, original_pay).await;

    finalize_period(&db, &employer_id, march()).await;
    finalize_period(&db, &employer_id, april()).await;
    finalize_period(&db, &employer_id, may()).await;

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
        &db,
        &employment_id,
        march().start(),
        april().start(),
        original_pay,
        &[march(), april(), may()],
        "March rate captured wrong; the terms only took effect in April",
        "actor",
    )
    .await
    .unwrap();

    assert_eq!(diverging, vec![march(), april(), may()]);

    // Insert March's true amount as a brand new row. March is already
    // finalized and live, so this insert is a correction too: it takes over
    // the span `[1 March, 1 April)` the move just freed, and must name and
    // have that one period acknowledged.
    let true_march_pay = Money::from_cents(550000).unwrap();
    let insert_diverging = record_compensation_terms(
        &db,
        &employment_id,
        march().start(),
        true_march_pay,
        &[march()],
        "March's true rate, recorded over the period it was already paid for",
        "actor",
    )
    .await
    .unwrap();
    assert_eq!(
        insert_diverging,
        vec![march()],
        "the insert governs 1 March up to the moved row, and March is live"
    );

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

    // Both halves of the split are logged, each naming its own divergence.
    let contexts = correction_contexts(&pool, &employment_id).await;
    assert_eq!(contexts.len(), 2, "a split is two acts, so two entries");

    let moved = &contexts[0];
    assert_eq!(
        moved["reason"],
        "March rate captured wrong; the terms only took effect in April"
    );
    assert_eq!(moved["before"]["effective_from"], "2026-03-01");
    assert_eq!(moved["after"]["effective_from"], "2026-04-01");
    assert_eq!(
        moved["diverging_live_finalized_periods"],
        serde_json::json!([
            { "period_start": "2026-03-01", "period_end": "2026-03-31" },
            { "period_start": "2026-04-01", "period_end": "2026-04-30" },
            { "period_start": "2026-05-01", "period_end": "2026-05-31" },
        ])
    );

    let inserted = &contexts[1];
    assert_eq!(
        inserted["reason"],
        "March's true rate, recorded over the period it was already paid for"
    );
    assert_eq!(
        inserted["before"],
        serde_json::Value::Null,
        "no row governed March under this key before the insert, and the \
         absence is recorded as an absence"
    );
    assert_eq!(inserted["after"]["effective_from"], "2026-03-01");
    assert_eq!(inserted["after"]["basic_pay_cents"], 550000);
    assert_eq!(
        inserted["diverging_live_finalized_periods"],
        serde_json::json!([{ "period_start": "2026-03-01", "period_end": "2026-03-31" }])
    );
}

/// Moving a row past a later sibling changes two disjoint spans: its old
/// span, and the span it starts governing at the new date. Both must be named
/// to the caller and ActionLog.
#[sqlx::test]
async fn moving_a_row_past_a_later_sibling_names_both_affected_spans(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id =
        an_employment_with_basic_pay(&db, &employer_id, Money::from_cents(500000).unwrap()).await;

    record_compensation_terms(
        &db,
        &employment_id,
        may().start(),
        Money::from_cents(600000).unwrap(),
        &[],
        "",
        "actor",
    )
    .await
    .unwrap();
    for period in [march(), april(), may(), june()] {
        finalize_period(&db, &employer_id, period).await;
    }

    let diverging = correct_compensation_terms(
        &db,
        &employment_id,
        march().start(),
        june().start(),
        Money::from_cents(700000).unwrap(),
        &[march(), april(), june()],
        "the earlier terms began in June",
        "actor",
    )
    .await
    .unwrap();

    assert_eq!(diverging, vec![march(), april(), june()]);
}

// ---- `declare_unsupported_deduction_status`'s reason and divergence ----

#[sqlx::test]
async fn an_empty_reason_is_refused_for_an_unsupported_deduction_declaration(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id =
        an_employment_with_basic_pay(&db, &employer_id, Money::from_cents(500000).unwrap()).await;

    let result = declare_unsupported_deduction_status(
        &db,
        &employment_id,
        april().start(),
        UnsupportedDeductionStatus::ConfirmedNone,
        &[],
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
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id =
        an_employment_with_basic_pay(&db, &employer_id, Money::from_cents(500000).unwrap()).await;

    finalize_period(&db, &employer_id, march()).await;

    let diverging = declare_unsupported_deduction_status(
        &db,
        &employment_id,
        march().start(),
        UnsupportedDeductionStatus::ConfirmedNone,
        &[march()],
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

// ---- §6.5 guard 2: the divergence must be acknowledged ----

/// The divergence list reaches the caller through the refusal itself: a
/// correction that names no acknowledgement is refused, told exactly which
/// Live finalized periods it diverges from, and leaves the row and the
/// ActionLog untouched. Re-asking with that list acknowledged carries the
/// identical correction through — divergence warns, it never refuses.
#[sqlx::test]
async fn an_unacknowledged_divergence_is_refused_and_names_the_periods(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let original_pay = Money::from_cents(500000).unwrap();
    let employment_id = an_employment_with_basic_pay(&db, &employer_id, original_pay).await;

    finalize_period(&db, &employer_id, march()).await;
    finalize_period(&db, &employer_id, april()).await;

    let corrected_pay = Money::from_cents(550000).unwrap();
    let result = correct_compensation_terms(
        &db,
        &employment_id,
        march().start(),
        march().start(),
        corrected_pay,
        &[],
        "March rate captured wrong",
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::MasterDataDivergenceNotAcknowledged {
            employment_id: employment_id.clone(),
            diverging_periods: vec![march(), april()],
        })
    );
    assert_eq!(
        basic_pay_cents_at(&pool, &employment_id, march().start()).await,
        500000,
        "a refused correction changes nothing"
    );
    assert_eq!(correction_entry_count(&pool, &employment_id).await, 0);

    // The same correction, now acknowledging exactly what it was told.
    let diverging = correct_compensation_terms(
        &db,
        &employment_id,
        march().start(),
        march().start(),
        corrected_pay,
        &[march(), april()],
        "March rate captured wrong",
        "actor",
    )
    .await
    .unwrap();

    assert_eq!(diverging, vec![march(), april()]);
    assert_eq!(
        basic_pay_cents_at(&pool, &employment_id, march().start()).await,
        550000
    );
    assert_eq!(correction_entry_count(&pool, &employment_id).await, 1);
}

/// An acknowledgement is of *this* list, not of divergence in general: one
/// that names only some of the periods is refused exactly as an empty one
/// is, so a caller cannot acknowledge a shorter list than the user saw.
#[sqlx::test]
async fn an_acknowledgement_that_omits_a_diverging_period_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id =
        an_employment_with_basic_pay(&db, &employer_id, Money::from_cents(500000).unwrap()).await;

    finalize_period(&db, &employer_id, march()).await;
    finalize_period(&db, &employer_id, april()).await;

    let result = correct_compensation_terms(
        &db,
        &employment_id,
        march().start(),
        march().start(),
        Money::from_cents(550000).unwrap(),
        &[march()],
        "March rate captured wrong",
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::MasterDataDivergenceNotAcknowledged {
            employment_id: employment_id.clone(),
            diverging_periods: vec![march(), april()],
        })
    );
    assert_eq!(correction_entry_count(&pool, &employment_id).await, 0);
}

/// Acknowledging a period that does not diverge is refused too. The
/// acknowledgement is the caller repeating back the list it was shown, so a
/// list it was never shown is not an acknowledgement of anything.
#[sqlx::test]
async fn an_acknowledgement_of_a_period_that_does_not_diverge_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id =
        an_employment_with_basic_pay(&db, &employer_id, Money::from_cents(500000).unwrap()).await;

    let result = correct_compensation_terms(
        &db,
        &employment_id,
        march().start(),
        march().start(),
        Money::from_cents(550000).unwrap(),
        &[march()],
        "March rate captured wrong",
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::MasterDataDivergenceNotAcknowledged {
            employment_id: employment_id.clone(),
            diverging_periods: Vec::new(),
        })
    );
}

/// The `UnsupportedDeductionStatus` declaration gets the same treatment
/// (§6.5, acceptance criterion 8): an unacknowledged change over a Live
/// finalized period is refused, and nothing is written.
#[sqlx::test]
async fn an_unacknowledged_declaration_change_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id =
        an_employment_with_basic_pay(&db, &employer_id, Money::from_cents(500000).unwrap()).await;

    finalize_period(&db, &employer_id, march()).await;

    let result = declare_unsupported_deduction_status(
        &db,
        &employment_id,
        march().start(),
        UnsupportedDeductionStatus::Present(
            UnsupportedDeductionKinds::new(vec![UnsupportedDeductionKind::ProvidentFund]).unwrap(),
        ),
        &[],
        "found a provident fund deduction we missed",
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::MasterDataDivergenceNotAcknowledged {
            employment_id: employment_id.clone(),
            diverging_periods: vec![march()],
        })
    );
    assert_eq!(
        get_unsupported_deduction_status(&db, &employment_id, march().end())
            .await
            .unwrap(),
        UnsupportedDeductionStatus::ConfirmedNone,
        "a refused declaration leaves the one already in force"
    );
}

// ---- §6.5 guard 3: nothing arithmetic can move ----

/// Acceptance criterion 6, asserted directly rather than trusted: a
/// correction of the very row March, April and May all read moves neither
/// their frozen figures nor June's `YearToDateContext`, because year-to-date
/// sums frozen numeric columns and never master data (ADR-0012).
#[sqlx::test]
async fn a_correction_moves_no_finalized_figure_and_no_year_to_date_total(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id =
        an_employment_with_basic_pay(&db, &employer_id, Money::from_cents(500000).unwrap()).await;

    for period in [march(), april(), may()] {
        finalize_period(&db, &employer_id, period).await;
    }

    let june_context_before = build_year_to_date_context(&db, &employment_id, june().end())
        .await
        .unwrap();
    let fingerprints_before = frozen_figures(&pool, &employment_id).await;
    assert_ne!(
        june_context_before.prior_taxable_remuneration(),
        Money::from_cents(0).unwrap(),
        "the totals under test must be non-zero, or this proves nothing"
    );

    // Double the salary every one of those three periods was calculated
    // from. Nothing already finalized may notice.
    let diverging = correct_compensation_terms(
        &db,
        &employment_id,
        march().start(),
        march().start(),
        Money::from_cents(1000000).unwrap(),
        &[march(), april(), may()],
        "the salary was captured at half its true value",
        "actor",
    )
    .await
    .unwrap();
    assert_eq!(diverging, vec![march(), april(), may()]);

    assert_eq!(
        build_year_to_date_context(&db, &employment_id, june().end())
            .await
            .unwrap(),
        june_context_before,
        "a master-data correction moves no year-to-date total"
    );
    assert_eq!(
        frozen_figures(&pool, &employment_id).await,
        fingerprints_before,
        "a master-data correction moves no finalized figure"
    );
}

/// A correction that would move a row onto a date this Employment already
/// has one at is a domain refusal naming the collision, never a raw database
/// error from `UNIQUE (employment_id, effective_from)`.
#[sqlx::test]
async fn moving_a_row_onto_a_date_that_already_has_one_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id =
        an_employment_with_basic_pay(&db, &employer_id, Money::from_cents(500000).unwrap()).await;
    record_compensation_terms(
        &db,
        &employment_id,
        may().start(),
        Money::from_cents(600000).unwrap(),
        &[],
        "",
        "actor",
    )
    .await
    .unwrap();

    let result = correct_compensation_terms(
        &db,
        &employment_id,
        march().start(),
        may().start(),
        Money::from_cents(550000).unwrap(),
        &[],
        "the terms began in May",
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::CompensationTermsAlreadyExistAt {
            employment_id: employment_id.clone(),
            effective_from: may().start(),
        })
    );
    assert_eq!(compensation_terms_row_count(&pool, &employment_id).await, 2);
    assert_eq!(
        basic_pay_cents_at(&pool, &employment_id, march().start()).await,
        500000
    );
}

/// §6.5's "March correction, end to end", as one test: correct the master
/// data by splitting the row, reverse March, replace it through a
/// CorrectionRun, and check what each step was for.
///
/// The point of the whole ticket is here. The correction is what lets the
/// replacement calculate from a corrected fact (§6.5 step 4, spec 76), the
/// split is what leaves April and May reading the `BasicPay` they always
/// read (step 6, spec 82), and only the Reversal-plus-Replacement moves a
/// figure — the correction in step 1 moved none (guard 3, spec 81).
#[sqlx::test]
async fn the_march_correction_end_to_end(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let wrong_pay = Money::from_cents(500000).unwrap();
    let employment_id = an_employment_with_basic_pay(&db, &employer_id, wrong_pay).await;

    let march_run_id =
        create_ordinary_payroll_run(&db, &employer_id, march(), march().end(), "actor")
            .await
            .unwrap();
    calculate_payroll_run(&db, &march_run_id, "calculator")
        .await
        .unwrap();
    let march_original = finalize_payroll_run(&db, &march_run_id, "finalizer")
        .await
        .unwrap()
        .finalized
        .into_iter()
        .find(|(id, _)| id == &employment_id)
        .expect("March must have finalized")
        .1;
    finalize_period(&db, &employer_id, april()).await;
    finalize_period(&db, &employer_id, may()).await;

    let april_before = finalized_payroll_fingerprint(&pool, &employment_id, april().end()).await;
    let may_before = finalized_payroll_fingerprint(&pool, &employment_id, may().end()).await;
    let june_context_before = build_year_to_date_context(&db, &employment_id, june().end())
        .await
        .unwrap();

    // 1. Correct master data: the terms only took effect in April, and March
    //    was always a different, higher amount.
    let diverging = correct_compensation_terms(
        &db,
        &employment_id,
        march().start(),
        april().start(),
        wrong_pay,
        &[march(), april(), may()],
        "March rate captured wrong; these terms began in April",
        "actor",
    )
    .await
    .unwrap();
    assert_eq!(diverging, vec![march(), april(), may()]);

    let true_march_pay = Money::from_cents(550000).unwrap();
    let insert_diverging = record_compensation_terms(
        &db,
        &employment_id,
        march().start(),
        true_march_pay,
        &[march()],
        "March's true rate, over the period already paid at the wrong one",
        "actor",
    )
    .await
    .unwrap();
    assert_eq!(insert_diverging, vec![march()]);

    // Nothing has moved yet. Correcting master data is not paying anybody.
    assert_eq!(
        build_year_to_date_context(&db, &employment_id, june().end())
            .await
            .unwrap(),
        june_context_before,
    );

    // 2, 3, 4, 5. Reverse March and replace it through a CorrectionRun,
    //    which assembles its PayrollInput from the corrected master data.
    reverse_finalized_payroll(&db, &march_original, "March was calculated wrong", "actor")
        .await
        .unwrap();
    let correction_run_id = create_correction_run(
        &db,
        &employer_id,
        march(),
        date(2026, 6, 5),
        "March salary was captured wrong",
        "actor",
    )
    .await
    .unwrap();
    add_employment_to_correction_run(
        &db,
        &correction_run_id,
        &employment_id,
        Some(&march_original),
        "actor",
    )
    .await
    .unwrap();
    assert_eq!(
        calculate_payroll_run(&db, &correction_run_id, "calculator")
            .await
            .unwrap(),
        Vec::new(),
    );
    finalize_payroll_run(&db, &correction_run_id, "finalizer")
        .await
        .unwrap();

    // The replacement calculated from the corrected fact: March's only
    // earning is its BasicPay, so its frozen TaxableRemuneration moved by
    // exactly the amount the correction added to it.
    let march_replacement_taxable: i64 = sqlx::query_scalar(
        "SELECT finalized.taxable_remuneration
         FROM live_finalized_payroll AS live
         JOIN finalized_payroll AS finalized ON finalized.id = live.finalized_payroll_id
         WHERE live.employment_id = $1 AND live.period_end = $2",
    )
    .bind(employment_id.as_str())
    .bind(march().end())
    .fetch_one(&pool)
    .await
    .unwrap();
    let march_original_taxable: i64 = sqlx::query_scalar(
        "SELECT taxable_remuneration FROM finalized_payroll WHERE id = $1::uuid",
    )
    .bind(march_original.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        march_replacement_taxable - march_original_taxable,
        true_march_pay.cents() - wrong_pay.cents(),
    );

    // 6. April and May are untouched, and June's year-to-date now picks up
    //    the Replacement rather than the reversed record.
    assert_eq!(
        finalized_payroll_fingerprint(&pool, &employment_id, april().end()).await,
        april_before,
    );
    assert_eq!(
        finalized_payroll_fingerprint(&pool, &employment_id, may().end()).await,
        may_before,
    );
    let june_context_after = build_year_to_date_context(&db, &employment_id, june().end())
        .await
        .unwrap();
    assert!(
        june_context_after.prior_taxable_remuneration()
            > june_context_before.prior_taxable_remuneration(),
        "only the Reversal plus Replacement moves a figure, and this one did"
    );
}

// ---- Issue #37: an insert is a correction wherever it diverges ----
//
// `record_compensation_terms` was the last way to make master data disagree
// with paid history in silence. It now computes, requires the acknowledgement
// of, and logs exactly the same divergence the two correction paths do — but
// only where there is one. An insert ahead of payroll is untouched.

/// Acceptance criterion 3: the ordinary act stays ordinary. A pay rise stated
/// from a date nothing has been paid for yet diverges from nothing, needs no
/// reason, acknowledges an empty list, and is not logged as a correction.
#[sqlx::test]
async fn recording_a_rise_ahead_of_payroll_diverges_from_nothing_and_needs_no_reason(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id =
        an_employment_with_basic_pay(&db, &employer_id, Money::from_cents(500000).unwrap()).await;

    finalize_period(&db, &employer_id, march()).await;

    let diverging = record_compensation_terms(
        &db,
        &employment_id,
        april().start(),
        Money::from_cents(600000).unwrap(),
        &[],
        "",
        "actor",
    )
    .await
    .unwrap();

    assert_eq!(
        diverging,
        Vec::new(),
        "April has never been finalized, so this insert disagrees with nothing"
    );
    assert_eq!(compensation_terms_row_count(&pool, &employment_id).await, 2);
    assert_eq!(
        correction_entry_count(&pool, &employment_id).await,
        0,
        "an insert that diverges from nothing is not a correction"
    );
}

/// An insert that lands on a live finalized span names it. Asked with nothing
/// acknowledged — the way a caller learns the list at all — it is refused,
/// and no row is written.
#[sqlx::test]
async fn an_unacknowledged_insert_over_a_live_finalized_period_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id =
        an_employment_with_basic_pay(&db, &employer_id, Money::from_cents(500000).unwrap()).await;

    // The Employment's only row starts in March, so 1 April is free to
    // insert on — and everything from April on is already paid.
    for period in [march(), april(), may()] {
        finalize_period(&db, &employer_id, period).await;
    }

    let result = record_compensation_terms(
        &db,
        &employment_id,
        april().start(),
        Money::from_cents(600000).unwrap(),
        &[],
        "",
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::MasterDataDivergenceNotAcknowledged {
            employment_id: employment_id.clone(),
            diverging_periods: vec![april(), may()],
        }),
        "the insert governs 1 April onwards, and April and May are both live"
    );
    assert_eq!(
        compensation_terms_row_count(&pool, &employment_id).await,
        1,
        "a refused insert writes no row"
    );
    assert_eq!(correction_entry_count(&pool, &employment_id).await, 0);
}

/// The acknowledgement is checked before the reason, so a caller asking to
/// *learn* the list is told the list rather than sent away for a sentence it
/// has no reason to write yet. Once the list is acknowledged, the reason is
/// demanded — and until it arrives, nothing is written.
#[sqlx::test]
async fn an_acknowledged_insert_over_a_live_finalized_period_still_demands_a_reason(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id =
        an_employment_with_basic_pay(&db, &employer_id, Money::from_cents(500000).unwrap()).await;

    finalize_period(&db, &employer_id, march()).await;
    finalize_period(&db, &employer_id, april()).await;

    let result = record_compensation_terms(
        &db,
        &employment_id,
        april().start(),
        Money::from_cents(600000).unwrap(),
        &[april()],
        "   ",
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::CompensationTermsCorrectionReasonCannotBeEmpty)
    );
    assert_eq!(
        compensation_terms_row_count(&pool, &employment_id).await,
        1,
        "a refused insert writes no row"
    );
    assert_eq!(correction_entry_count(&pool, &employment_id).await, 0);
}

/// The divergence span of an insert ends at the next row that already exists,
/// exactly as `declare_unsupported_deduction_status` derives its own — so a
/// later sibling keeps its own periods out of this insert's list.
#[sqlx::test]
async fn an_inserts_divergence_stops_at_the_next_row_that_already_exists(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id =
        an_employment_with_basic_pay(&db, &employer_id, Money::from_cents(500000).unwrap()).await;
    record_compensation_terms(
        &db,
        &employment_id,
        may().start(),
        Money::from_cents(600000).unwrap(),
        &[],
        "",
        "actor",
    )
    .await
    .unwrap();
    for period in [march(), april(), may(), june()] {
        finalize_period(&db, &employer_id, period).await;
    }

    // Inserting at 1 April takes `[1 April, 1 May)` — May's own row already
    // governs from there, so May and June belong to it, not to this insert.
    let diverging = record_compensation_terms(
        &db,
        &employment_id,
        april().start(),
        Money::from_cents(550000).unwrap(),
        &[april()],
        "April's true rate",
        "actor",
    )
    .await
    .unwrap();

    assert_eq!(diverging, vec![april()]);

    let contexts = correction_contexts(&pool, &employment_id).await;
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0]["reason"], "April's true rate");
    assert_eq!(contexts[0]["before"], serde_json::Value::Null);
    assert_eq!(contexts[0]["after"]["effective_from"], "2026-04-01");
    assert_eq!(contexts[0]["after"]["basic_pay_cents"], 550000);
    assert_eq!(
        contexts[0]["diverging_live_finalized_periods"],
        serde_json::json!([{ "period_start": "2026-04-01", "period_end": "2026-04-30" }])
    );
}

/// Acknowledging a period this insert does not diverge from is refused too:
/// the acknowledgement must be of exactly the list, not merely a superset of
/// it, or the ActionLog would record agreement to something else.
#[sqlx::test]
async fn an_insert_acknowledging_a_period_it_does_not_diverge_from_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id =
        an_employment_with_basic_pay(&db, &employer_id, Money::from_cents(500000).unwrap()).await;

    finalize_period(&db, &employer_id, march()).await;

    let result = record_compensation_terms(
        &db,
        &employment_id,
        april().start(),
        Money::from_cents(600000).unwrap(),
        &[march()],
        "a reason",
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::MasterDataDivergenceNotAcknowledged {
            employment_id: employment_id.clone(),
            diverging_periods: Vec::new(),
        }),
        "March is before this insert's span; it diverges from nothing"
    );
    assert_eq!(compensation_terms_row_count(&pool, &employment_id).await, 1);
}

/// Acceptance criterion 6 for the insert path, asserted directly rather than
/// trusted: year-to-date sums frozen numeric columns and never master data
/// (ADR-0012), so an insert over live finalized periods cannot move a cent.
#[sqlx::test]
async fn an_insert_moves_no_finalized_figure_and_no_year_to_date_total(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id =
        an_employment_with_basic_pay(&db, &employer_id, Money::from_cents(500000).unwrap()).await;

    for period in [march(), april(), may()] {
        finalize_period(&db, &employer_id, period).await;
    }

    let june_context_before = build_year_to_date_context(&db, &employment_id, june().end())
        .await
        .unwrap();
    let figures_before = frozen_figures(&pool, &employment_id).await;
    assert_ne!(
        june_context_before.prior_taxable_remuneration(),
        Money::from_cents(0).unwrap(),
        "the totals under test must be non-zero, or this proves nothing"
    );

    // Insert a row governing April and May at double the salary they were
    // both calculated from. Neither may notice.
    let diverging = record_compensation_terms(
        &db,
        &employment_id,
        april().start(),
        Money::from_cents(1000000).unwrap(),
        &[april(), may()],
        "the April rise was never recorded",
        "actor",
    )
    .await
    .unwrap();
    assert_eq!(diverging, vec![april(), may()]);

    assert_eq!(
        build_year_to_date_context(&db, &employment_id, june().end())
            .await
            .unwrap(),
        june_context_before,
        "an insert over live finalized periods moves no year-to-date total"
    );
    assert_eq!(
        frozen_figures(&pool, &employment_id).await,
        figures_before,
        "an insert over live finalized periods moves no finalized figure"
    );
}

/// Acceptance criterion 1's word is **Live**. A period whose
/// `FinalizedPayroll` has been reversed is no longer live, so an insert over
/// it disagrees with nothing anybody is still relying on and must not name
/// it — nor demand it be acknowledged.
///
/// The helper reads `live_finalized_payroll` and never `finalized_payroll`,
/// so this holds structurally; it is asserted here because the insert path
/// is where the claim is newly made.
#[sqlx::test]
async fn an_insert_never_names_a_reversed_period_it_covers(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id =
        an_employment_with_basic_pay(&db, &employer_id, Money::from_cents(500000).unwrap()).await;

    finalize_period(&db, &employer_id, march()).await;
    let april_original =
        finalize_period_returning_id(&db, &employer_id, &employment_id, april()).await;
    finalize_period(&db, &employer_id, may()).await;

    reverse_finalized_payroll(&db, &april_original, "April was paid wrong", "actor")
        .await
        .unwrap();

    // The insert governs 1 April onwards. April's record is reversed and May's
    // is not, so exactly one of the two periods it covers is a divergence.
    let diverging = record_compensation_terms(
        &db,
        &employment_id,
        april().start(),
        Money::from_cents(600000).unwrap(),
        &[may()],
        "the April rise was never recorded",
        "actor",
    )
    .await
    .unwrap();

    assert_eq!(
        diverging,
        vec![may()],
        "a reversed period is not Live, so the insert diverges from May alone"
    );

    let contexts = correction_contexts(&pool, &employment_id).await;
    assert_eq!(contexts.len(), 1);
    assert_eq!(
        contexts[0]["diverging_live_finalized_periods"],
        serde_json::json!([{ "period_start": "2026-05-01", "period_end": "2026-05-31" }]),
        "the ActionLog records the same Live-only list the caller acknowledged"
    );
}

/// Acknowledging a reversed period is refused for the same reason
/// acknowledging any non-diverging period is: the entry would record
/// agreement to a list that was never true.
#[sqlx::test]
async fn an_insert_acknowledging_a_reversed_period_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let employment_id =
        an_employment_with_basic_pay(&db, &employer_id, Money::from_cents(500000).unwrap()).await;

    finalize_period(&db, &employer_id, march()).await;
    let april_original =
        finalize_period_returning_id(&db, &employer_id, &employment_id, april()).await;

    reverse_finalized_payroll(&db, &april_original, "April was paid wrong", "actor")
        .await
        .unwrap();

    let result = record_compensation_terms(
        &db,
        &employment_id,
        april().start(),
        Money::from_cents(600000).unwrap(),
        &[april()],
        "the April rise was never recorded",
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::MasterDataDivergenceNotAcknowledged {
            employment_id: employment_id.clone(),
            diverging_periods: Vec::new(),
        }),
    );
    assert_eq!(compensation_terms_row_count(&pool, &employment_id).await, 1);
    assert_eq!(correction_entry_count(&pool, &employment_id).await, 0);
}
