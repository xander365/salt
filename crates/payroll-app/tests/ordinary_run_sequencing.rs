//! Proves issue #34: `finalize_payroll_run`'s Ordinary preceding-period
//! check — `docs/domain/payroll-run-persistence.md` §5.3 step 3 and §7 —
//! reached through the public API, never raw SQL, except where a scenario
//! needs a fact the public API cannot yet produce.
//!
//! Every test below uses a monthly, calendar-month `PaySchedule`, so a
//! period's own predecessor is simply the previous calendar month.

use chrono::NaiveDate;
use payroll::{
    EmployerId, EmploymentId, Money, PayPeriod, PeriodEndDay, PersonId, PriorEmployment, TaxYear,
    UnsupportedDeductionStatus,
};
use payroll_app::{
    PayrollAppError, PayrollRunId, calculate_payroll_run, create_employer, create_employment,
    create_ordinary_payroll_run, declare_prior_employment, declare_unsupported_deduction_status,
    finalize_payroll_run, record_compensation_terms, record_opening_balance,
    remove_employment_from_run, reverse_finalized_payroll,
};
use sqlx::PgPool;

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn monthly_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::LastDayOfMonth)
}

/// The calendar month `year`/`month` as a `PayPeriod`, under
/// `monthly_schedule()`.
fn month_period(year: i32, month: u32) -> PayPeriod {
    let start = date(year, month, 1);
    let end = if month == 12 {
        date(year + 1, 1, 1)
    } else {
        date(year, month + 1, 1)
    }
    .pred_opt()
    .unwrap();
    PayPeriod::new(start, end).unwrap()
}

async fn an_employer(pool: &PgPool) -> EmployerId {
    create_employer(pool, monthly_schedule(), "actor")
        .await
        .unwrap()
}

/// Creates an Employment starting `start_date`, with every fact `calculate`
/// needs already on record from `start_date` onward: `CompensationTerms`
/// effective that day, a confirmed absence of `PriorEmployment` for
/// `tax_year`, and a confirmed absence of unsupported deductions from that
/// day. No `OpeningBalance` — callers that need one record it themselves.
async fn a_fully_declared_employment(
    pool: &PgPool,
    employer_id: &EmployerId,
    person: &str,
    start_date: NaiveDate,
    tax_year: TaxYear,
    basic_pay: Money,
) -> EmploymentId {
    let employment_id = create_employment(
        pool,
        employer_id,
        &PersonId::new(person),
        start_date,
        None,
        "actor",
    )
    .await
    .unwrap();
    record_compensation_terms(pool, &employment_id, start_date, basic_pay, "actor")
        .await
        .unwrap();
    declare_prior_employment(
        pool,
        &employment_id,
        tax_year,
        PriorEmployment::None,
        "actor",
    )
    .await
    .unwrap();
    declare_unsupported_deduction_status(
        pool,
        &employment_id,
        start_date,
        UnsupportedDeductionStatus::ConfirmedNone,
        &[],
        "a reason",
        "actor",
    )
    .await
    .unwrap();
    employment_id
}

/// Creates an Ordinary run for `period`, calculates it, and finalizes it —
/// the run's members are whoever the Employer's active Employments overlap
/// `period` with, exactly as `create_ordinary_payroll_run` auto-proposes.
async fn finalize_period(
    pool: &PgPool,
    employer_id: &EmployerId,
    period: PayPeriod,
    pay_date: NaiveDate,
) -> Result<PayrollRunId, PayrollAppError> {
    let run_id = create_ordinary_payroll_run(pool, employer_id, period, pay_date, "actor").await?;
    let refusals = calculate_payroll_run(pool, &run_id, "calculator").await?;
    assert_eq!(refusals, Vec::new(), "the run must reach Calculated");
    finalize_payroll_run(pool, &run_id, "finalizer").await?;
    Ok(run_id)
}

// ---- §7.1 branch 1: outside the Employment ----

#[sqlx::test]
async fn branch_1_an_employment_starting_after_the_preceding_period_resolves_it(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    // Starts 1 April: March, the preceding period, never overlaps it.
    a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        date(2026, 4, 1),
        TaxYear::starting(2026),
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;

    let result =
        finalize_period(&pool, &employer_id, month_period(2026, 4), date(2026, 5, 5)).await;

    assert!(result.is_ok(), "expected April to finalize, got {result:?}");
}

// ---- §7.1 branch 2: before Salt ----

#[sqlx::test]
async fn branch_2_an_opening_balance_boundary_after_the_preceding_period_resolves_it(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        date(2026, 3, 1),
        TaxYear::starting(2026),
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    // Salt is responsible starting with April's own period: March's figures
    // are inside this balance.
    record_opening_balance(
        &pool,
        &employment_id,
        TaxYear::starting(2026),
        date(2026, 4, 30),
        Money::from_cents(1_000_000).unwrap(),
        Money::from_cents(100_000).unwrap(),
        "actor",
    )
    .await
    .unwrap();

    let result =
        finalize_period(&pool, &employer_id, month_period(2026, 4), date(2026, 5, 5)).await;

    assert!(result.is_ok(), "expected April to finalize, got {result:?}");
}

// ---- §7.1 branch 3: paid ----

#[sqlx::test]
async fn branch_3_a_live_finalized_payroll_for_the_preceding_period_resolves_it(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        date(2026, 3, 1),
        TaxYear::starting(2026),
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    // March is the TaxYear's own first period, so it finalizes with nothing
    // to check.
    finalize_period(&pool, &employer_id, month_period(2026, 3), date(2026, 4, 5))
        .await
        .unwrap();

    let result =
        finalize_period(&pool, &employer_id, month_period(2026, 4), date(2026, 5, 5)).await;

    assert!(result.is_ok(), "expected April to finalize, got {result:?}");
}

// ---- §7.1 branch 4, first half: reasoned removal ----

#[sqlx::test]
async fn branch_4a_a_reasoned_removal_from_the_finalized_ordinary_run_resolves_it(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        date(2026, 3, 1),
        TaxYear::starting(2026),
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;

    // March is auto-proposed, then removed with a reason before it
    // calculates — the run still finalizes, vacuously.
    let march_run_id = create_ordinary_payroll_run(
        &pool,
        &employer_id,
        month_period(2026, 3),
        date(2026, 4, 5),
        "actor",
    )
    .await
    .unwrap();
    remove_employment_from_run(
        &pool,
        &march_run_id,
        &employment_id,
        "on unpaid leave all of March",
        "actor",
    )
    .await
    .unwrap();
    let refusals = calculate_payroll_run(&pool, &march_run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(refusals, Vec::new());
    finalize_payroll_run(&pool, &march_run_id, "finalizer")
        .await
        .unwrap();

    // The Employment is still active, so it is a member of April too.
    let result =
        finalize_period(&pool, &employer_id, month_period(2026, 4), date(2026, 5, 5)).await;

    assert!(result.is_ok(), "expected April to finalize, got {result:?}");
}

/// The other side of branch 4's first half: a reasoned removal only resolves
/// the period once the run it was made in has itself **finalized** (§4.8). A
/// removal from a run still sitting in `Draft` or `Calculated` says nothing
/// yet — that run may still be recalculated with the member added back, or
/// never finalized at all — so April must still refuse.
#[sqlx::test]
async fn branch_4a_a_removal_from_a_run_that_never_finalized_does_not_resolve_it(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        date(2026, 3, 1),
        TaxYear::starting(2026),
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;

    // March is created and the member removed with a reason, exactly as in
    // the test above — but March is left open, never finalized.
    let march_run_id = create_ordinary_payroll_run(
        &pool,
        &employer_id,
        month_period(2026, 3),
        date(2026, 4, 5),
        "actor",
    )
    .await
    .unwrap();
    remove_employment_from_run(
        &pool,
        &march_run_id,
        &employment_id,
        "on unpaid leave all of March",
        "actor",
    )
    .await
    .unwrap();

    let result =
        finalize_period(&pool, &employer_id, month_period(2026, 4), date(2026, 5, 5)).await;

    assert_eq!(
        result,
        Err(PayrollAppError::PrecedingPeriodUnresolved {
            employment_id,
            period: month_period(2026, 3),
        })
    );
}

// ---- §7.1 branch 4, second half: bare reversal ----

#[sqlx::test]
async fn branch_4b_a_reversed_finalized_payroll_with_none_live_resolves_it(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        date(2026, 3, 1),
        TaxYear::starting(2026),
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    let march_run_id = create_ordinary_payroll_run(
        &pool,
        &employer_id,
        month_period(2026, 3),
        date(2026, 4, 5),
        "actor",
    )
    .await
    .unwrap();
    let refusals = calculate_payroll_run(&pool, &march_run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(refusals, Vec::new());
    let finalized = finalize_payroll_run(&pool, &march_run_id, "finalizer")
        .await
        .unwrap()
        .finalized;
    let (_, finalized_payroll_id) = finalized
        .into_iter()
        .find(|(id, _)| *id == employment_id)
        .expect("the Employment must have finalized");
    reverse_finalized_payroll(
        &pool,
        &finalized_payroll_id,
        "the March figures were wrong, no replacement yet",
        "actor",
    )
    .await
    .unwrap();

    let result =
        finalize_period(&pool, &employer_id, month_period(2026, 4), date(2026, 5, 5)).await;

    assert!(result.is_ok(), "expected April to finalize, got {result:?}");
}

// ---- Absence of every record: the gap that must refuse ----

#[sqlx::test]
async fn absence_of_every_record_refuses_and_names_the_employment_and_the_period(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        date(2026, 3, 1),
        TaxYear::starting(2026),
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    // March is never run at all: no FinalizedPayroll, no removal, no
    // OpeningBalance, and the Employment did overlap it.

    let result =
        finalize_period(&pool, &employer_id, month_period(2026, 4), date(2026, 5, 5)).await;

    assert_eq!(
        result,
        Err(PayrollAppError::PrecedingPeriodUnresolved {
            employment_id,
            period: month_period(2026, 3),
        })
    );
}

/// The refused finalization must leave no history behind — the same
/// all-or-nothing guarantee every other finalization refusal gives (§5.1).
#[sqlx::test]
async fn a_refusal_leaves_no_finalized_payroll_and_no_status_change(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        date(2026, 3, 1),
        TaxYear::starting(2026),
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;

    let run_id = create_ordinary_payroll_run(
        &pool,
        &employer_id,
        month_period(2026, 4),
        date(2026, 5, 5),
        "actor",
    )
    .await
    .unwrap();
    let refusals = calculate_payroll_run(&pool, &run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(refusals, Vec::new());

    let result = finalize_payroll_run(&pool, &run_id, "finalizer").await;
    assert!(result.is_err());

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM finalized_payroll")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let status: String = sqlx::query_scalar("SELECT status FROM payroll_run WHERE id = $1::uuid")
        .bind(run_id.as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "calculated");
}

// ---- Exactly one period back, and never across the TaxYear boundary ----

/// A gap in the TaxYear *before* the one being finalized must never block
/// the TaxYear's own first period — the walk stops at the boundary and
/// never reaches it.
#[sqlx::test]
async fn the_walk_never_crosses_the_tax_year_boundary(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    // Employed well before this TaxYear, with a real, unresolved gap in
    // January 2026 (TaxYear 2025: January belongs to the year that started
    // the previous March) — no FinalizedPayroll, no removal, no
    // OpeningBalance for TaxYear 2025 at all.
    a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        date(2025, 1, 1),
        TaxYear::starting(2026),
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;

    // March 2026 is TaxYear 2026's own first period. Its predecessor,
    // February 2026, belongs to TaxYear 2025 — the walk stops there and
    // never looks at January.
    let result =
        finalize_period(&pool, &employer_id, month_period(2026, 3), date(2026, 4, 5)).await;

    assert!(
        result.is_ok(),
        "March must finalize despite the unresolved January gap in the prior \
         TaxYear, got {result:?}"
    );
}

/// A gap two periods back must not block finalization either, once the
/// immediate predecessor is itself resolved — only one period is ever read.
///
/// April is left a genuine, permanent gap: nothing ever resolves it. May is
/// resolved anyway, by a reasoned removal — which, unlike branches 1-3, asks
/// nothing about May's *own* predecessor. June's own check then only ever
/// asks about May, and June finalizing proves it never reached back through
/// May into April a second time.
///
/// `basic_pay` is kept low here so a genuinely skipped April cannot itself
/// cause June's *calculation* to refuse (ADR-0001, cumulative PAYE) for a
/// reason unrelated to what this test is about — a real April gap is
/// exactly the sort of history the tax engine is also entitled to notice;
/// this test isolates the sequencing refusal from that one.
#[sqlx::test]
async fn only_the_immediate_predecessor_is_read_not_a_gap_two_periods_back(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        date(2026, 3, 1),
        TaxYear::starting(2026),
        Money::from_cents(100_000).unwrap(),
    )
    .await;
    // March is the TaxYear's own first period: it finalizes with nothing to
    // check.
    finalize_period(&pool, &employer_id, month_period(2026, 3), date(2026, 4, 5))
        .await
        .unwrap();
    // April is skipped entirely: no FinalizedPayroll, no removal, no
    // OpeningBalance covering it — a permanent, never-resolved gap.

    // May is auto-proposed (the Employment is still active), then removed
    // with a reason before calculating: the run finalizes vacuously, and
    // that removal resolves May by branch 4 without May's own predecessor
    // (April) ever being asked about.
    let may_run_id = create_ordinary_payroll_run(
        &pool,
        &employer_id,
        month_period(2026, 5),
        date(2026, 6, 5),
        "actor",
    )
    .await
    .unwrap();
    remove_employment_from_run(
        &pool,
        &may_run_id,
        &employment_id,
        "still not paid, kept out deliberately",
        "actor",
    )
    .await
    .unwrap();
    let refusals = calculate_payroll_run(&pool, &may_run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(refusals, Vec::new());
    finalize_payroll_run(&pool, &may_run_id, "finalizer")
        .await
        .unwrap();

    // June's own predecessor is May, resolved by that removal (branch 4a).
    // If finalization ever walked a second period back it would reach
    // April's real gap and refuse; it must not.
    let result =
        finalize_period(&pool, &employer_id, month_period(2026, 6), date(2026, 7, 5)).await;
    assert!(
        result.is_ok(),
        "June must finalize despite April's real, unresolved gap two periods \
         back, got {result:?}"
    );
}

// ---- The A/B pair (§4.5, §7.3, §13) ----

/// An October `SaltCoverageStart` lets October finalize with September
/// unresolved by any Salt record at all.
#[sqlx::test]
async fn the_a_b_pair_an_october_boundary_lets_october_finalize(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        date(2026, 3, 1),
        TaxYear::starting(2026),
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    // Adopted Salt in October: everything before is pre-Salt and inside
    // this balance. No March-through-September PayrollRun ever exists.
    // Zero prior figures are the ordinary case over a non-empty span, and
    // keep October's own YearToDateContext trivially consistent with
    // periods_elapsed regardless of the seven periods this balance covers.
    record_opening_balance(
        &pool,
        &employment_id,
        TaxYear::starting(2026),
        date(2026, 10, 31),
        Money::ZERO,
        Money::ZERO,
        "actor",
    )
    .await
    .unwrap();

    let result = finalize_period(
        &pool,
        &employer_id,
        month_period(2026, 10),
        date(2026, 11, 5),
    )
    .await;

    assert!(
        result.is_ok(),
        "October must finalize: September is inside the OpeningBalance, got {result:?}"
    );
}

/// A frozen March `SaltCoverageStart` — adopted from day one, then a
/// forgotten September — makes the October run refuse.
///
/// `basic_pay` is kept low so October's own *calculation* succeeds despite
/// the real six-month gap between August (period 6) and October (period 8)
/// — it must reach `Calculated` so the finalization refusal under test is
/// this ticket's sequencing check (§7), not ADR-0001's unrelated cumulative
/// PAYE guard.
#[sqlx::test]
async fn the_a_b_pair_a_frozen_march_boundary_makes_october_refuse_a_skipped_september(
    pool: PgPool,
) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        date(2026, 3, 1),
        TaxYear::starting(2026),
        Money::from_cents(100_000).unwrap(),
    )
    .await;
    // Adopted from the very first period: an empty covered span is the
    // legitimate boundary here, and it freezes at March's finalization.
    record_opening_balance(
        &pool,
        &employment_id,
        TaxYear::starting(2026),
        date(2026, 3, 31),
        Money::ZERO,
        Money::ZERO,
        "actor",
    )
    .await
    .unwrap();

    // March through August all really ran and finalized.
    for month in 3..=8 {
        finalize_period(
            &pool,
            &employer_id,
            month_period(2026, month),
            date(2026, month + 1, 5),
        )
        .await
        .unwrap();
    }
    // September is skipped entirely — forgotten.

    let result = finalize_period(
        &pool,
        &employer_id,
        month_period(2026, 10),
        date(2026, 11, 5),
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::PrecedingPeriodUnresolved {
            employment_id,
            period: month_period(2026, 9),
        }),
        "October must refuse: September is a real, unrecorded gap"
    );
}

// ---- Mid-year onboarding with a backdated start date (§7.3 row 7) ----

/// An Employment onboarded in June with a March `start_date` refuses until
/// it is given a June boundary. The naive "does the Employment overlap"
/// check would wrongly think March overlaps nothing before it existed in
/// Salt — but `start_date` says the Employment existed since March, so
/// branch 1 does not apply, and nothing else resolves March, April or May
/// without an affirmative record.
#[sqlx::test]
async fn mid_year_onboarding_refuses_until_given_a_salt_coverage_start(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    // `start_date` is backdated to March, as if the person was truly hired
    // then, even though the Employer only onboarded to Salt in June and
    // back-declared every fact from March onward.
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        date(2026, 3, 1),
        TaxYear::starting(2026),
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;

    // §4.6: only one Ordinary run may ever exist for (Employer, June), so
    // the same run is calculated once and finalized twice — the second
    // attempt, after the fix below, is a real retry of the same run rather
    // than a fresh one.
    let june_run_id = create_ordinary_payroll_run(
        &pool,
        &employer_id,
        month_period(2026, 6),
        date(2026, 7, 5),
        "actor",
    )
    .await
    .unwrap();
    let refusals = calculate_payroll_run(&pool, &june_run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(refusals, Vec::new());

    let refused = finalize_payroll_run(&pool, &june_run_id, "finalizer").await;
    assert_eq!(
        refused,
        Err(PayrollAppError::PrecedingPeriodUnresolved {
            employment_id: employment_id.clone(),
            period: month_period(2026, 5),
        }),
        "June must refuse without a boundary or a back-fill"
    );

    // Given a June boundary — Salt is responsible starting with June's own
    // period — March through May are inside the balance. Zero prior figures
    // are the ordinary case over a non-empty span.
    record_opening_balance(
        &pool,
        &employment_id,
        TaxYear::starting(2026),
        date(2026, 6, 30),
        Money::ZERO,
        Money::ZERO,
        "actor",
    )
    .await
    .unwrap();

    let result = finalize_payroll_run(&pool, &june_run_id, "finalizer").await;
    assert!(
        result.is_ok(),
        "June must finalize once given a June SaltCoverageStart, got {result:?}"
    );
}

/// The other legitimate fix from the same starting point: back-filling
/// March, April and May with real, finalized runs instead of a boundary.
#[sqlx::test]
async fn mid_year_onboarding_refuses_until_back_filled(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    let employment_id = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        date(2026, 3, 1),
        TaxYear::starting(2026),
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;

    let june_run_id = create_ordinary_payroll_run(
        &pool,
        &employer_id,
        month_period(2026, 6),
        date(2026, 7, 5),
        "actor",
    )
    .await
    .unwrap();
    let refusals = calculate_payroll_run(&pool, &june_run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(refusals, Vec::new());

    let refused = finalize_payroll_run(&pool, &june_run_id, "finalizer").await;
    assert_eq!(
        refused,
        Err(PayrollAppError::PrecedingPeriodUnresolved {
            employment_id,
            period: month_period(2026, 5),
        })
    );

    // Back-fill March, April and May for real — no boundary at all.
    for month in 3..=5 {
        finalize_period(
            &pool,
            &employer_id,
            month_period(2026, month),
            date(2026, month + 1, 5),
        )
        .await
        .unwrap();
    }

    // March-May finalizing changed June's own YearToDateContext (it now
    // sums real prior figures instead of zero), so June's working
    // calculation is recomputed before finalizing again — otherwise §5.2's
    // own three-way equality would refuse it for a reason unrelated to this
    // test.
    let refusals = calculate_payroll_run(&pool, &june_run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(refusals, Vec::new());

    let result = finalize_payroll_run(&pool, &june_run_id, "finalizer").await;
    assert!(
        result.is_ok(),
        "June must finalize once March-May are back-filled, got {result:?}"
    );
}

// ---- Every member, not merely some member ----

/// §7.1 is asked once per member and every answer has to be yes. A run
/// holding one member the preceding period resolves for and one it does not
/// must refuse — and must name the member it refused for, not the run.
///
/// This is also the only place the batched reads behind the check are put in
/// front of more than one Employment at once: a resolved member must not
/// answer for an unresolved one.
#[sqlx::test]
async fn one_unresolved_member_refuses_the_whole_run_and_names_that_member(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    // Starts 1 April, so March — the preceding period — never overlaps it
    // and branch 1 resolves March for this one.
    a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        date(2026, 4, 1),
        TaxYear::starting(2026),
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    // Starts 1 March and March is never run at all: nothing resolves it.
    let skipped = a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-2",
        date(2026, 3, 1),
        TaxYear::starting(2026),
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;

    let result =
        finalize_period(&pool, &employer_id, month_period(2026, 4), date(2026, 5, 5)).await;

    assert_eq!(
        result,
        Err(PayrollAppError::PrecedingPeriodUnresolved {
            employment_id: skipped,
            period: month_period(2026, 3),
        })
    );

    // All or nothing (§5.1): the member March *was* resolved for gets no
    // FinalizedPayroll either.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM finalized_payroll")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

/// The same two members, each resolved by a different branch — one by
/// branch 1 and one by branch 3 — finalize together. Two members answered
/// from one set of reads, by two different branches, is the shape the
/// refusal above is the negative of.
#[sqlx::test]
async fn members_resolved_by_different_branches_finalize_together(pool: PgPool) {
    let employer_id = an_employer(&pool).await;
    a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-1",
        date(2026, 4, 1),
        TaxYear::starting(2026),
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    a_fully_declared_employment(
        &pool,
        &employer_id,
        "person-2",
        date(2026, 3, 1),
        TaxYear::starting(2026),
        Money::from_cents(1_500_000).unwrap(),
    )
    .await;
    // March holds person-2 alone — person-1 does not overlap it.
    finalize_period(&pool, &employer_id, month_period(2026, 3), date(2026, 4, 5))
        .await
        .unwrap();

    let result =
        finalize_period(&pool, &employer_id, month_period(2026, 4), date(2026, 5, 5)).await;

    assert!(result.is_ok(), "expected April to finalize, got {result:?}");

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM finalized_payroll WHERE period_end = $1")
            .bind(date(2026, 4, 30))
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 2, "both members finalize");
}
