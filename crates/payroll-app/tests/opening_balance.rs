//! Proves `record_opening_balance` (issue #28): the seam
//! `docs/domain/payroll-run-persistence.md` §4.5 and ADR-0014 describe,
//! reached through the public API rather than raw SQL.

use chrono::NaiveDate;
use payroll::{
    DayOfMonth, EmployerId, EmploymentId, Money, PeriodEndDay, PriorEmployment, TaxYear,
    UnsupportedDeductionStatus,
};
use payroll_app::{
    EmploymentPerson, PayrollAppError, SaltDatabase, create_employer, create_employment,
    declare_prior_employment, declare_unsupported_deduction_status, record_compensation_terms,
    record_opening_balance, void_employment,
};
use sqlx::{PgPool, Row};

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn calendar_month_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::LastDayOfMonth)
}

fn twenty_sixth_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap()))
}

async fn an_employer_and_employment(
    db: &SaltDatabase,
    schedule: payroll::PaySchedule,
    start_date: NaiveDate,
) -> (EmployerId, EmploymentId) {
    let employer_id = create_employer(db, "Employer", schedule, "actor")
        .await
        .unwrap();
    let (_, employment_id) = create_employment(
        db,
        &employer_id,
        EmploymentPerson::New("person-1".to_string()),
        start_date,
        None,
        "actor",
    )
    .await
    .unwrap();
    (employer_id, employment_id)
}

// ---- The legitimate mid-year adoption case (§4.5, ADR-0014 case A) ----

#[sqlx::test]
async fn a_mid_year_adoption_boundary_with_non_zero_prior_figures_is_recorded(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    // Employer E adopts Salt in October: the Employment existed all TaxYear
    // (started at the TaxYear's own first period, 1 March), and
    // March-September is pre-Salt.
    let (_, employment_id) =
        an_employer_and_employment(&db, calendar_month_schedule(), date(2026, 3, 1)).await;

    record_opening_balance(
        &db,
        &employment_id,
        TaxYear::starting(2026),
        date(2026, 10, 31),
        Money::from_cents(700_000).unwrap(),
        Money::from_cents(140_000).unwrap(),
        "actor",
    )
    .await
    .unwrap();

    let row = sqlx::query(
        "SELECT tax_year, first_salt_period_end, prior_taxable_remuneration, prior_paye
         FROM opening_balance WHERE employment_id = $1",
    )
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<i32, _>(0), 2026);
    assert_eq!(row.get::<NaiveDate, _>(1), date(2026, 10, 31));
    assert_eq!(row.get::<i64, _>(2), 700_000);
    assert_eq!(row.get::<i64, _>(3), 140_000);
}

#[sqlx::test]
async fn the_first_opening_balance_writes_a_created_action_log_entry(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (employer_id, employment_id) =
        an_employer_and_employment(&db, calendar_month_schedule(), date(2026, 3, 1)).await;

    record_opening_balance(
        &db,
        &employment_id,
        TaxYear::starting(2026),
        date(2026, 9, 30),
        Money::ZERO,
        Money::ZERO,
        "actor",
    )
    .await
    .unwrap();

    let row = sqlx::query(
        "SELECT employer_id, actor, action_type, target_type, target_id
         FROM action_log_entry WHERE target_id = $1",
    )
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>(0), employer_id.as_str());
    assert_eq!(row.get::<String, _>(1), "actor");
    assert_eq!(row.get::<String, _>(2), "opening_balance_created");
    assert_eq!(row.get::<String, _>(3), "employment");
}

/// The same (Employment, TaxYear) recorded twice replaces the row rather
/// than duplicating it, and the second write is a `Changed` act, not a
/// second `Created`.
#[sqlx::test]
async fn re_recording_replaces_the_row_and_writes_a_changed_entry(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id) =
        an_employer_and_employment(&db, calendar_month_schedule(), date(2026, 3, 1)).await;

    record_opening_balance(
        &db,
        &employment_id,
        TaxYear::starting(2026),
        date(2026, 9, 30),
        Money::from_cents(700_000).unwrap(),
        Money::from_cents(140_000).unwrap(),
        "actor",
    )
    .await
    .unwrap();

    record_opening_balance(
        &db,
        &employment_id,
        TaxYear::starting(2026),
        date(2026, 10, 31),
        Money::from_cents(800_000).unwrap(),
        Money::from_cents(160_000).unwrap(),
        "actor",
    )
    .await
    .unwrap();

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM opening_balance")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "one row per (Employment, TaxYear), not two");

    let row = sqlx::query(
        "SELECT first_salt_period_end, prior_taxable_remuneration
         FROM opening_balance WHERE employment_id = $1",
    )
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<NaiveDate, _>(0), date(2026, 10, 31));
    assert_eq!(row.get::<i64, _>(1), 800_000);

    let action_types: Vec<String> = sqlx::query_scalar(
        "SELECT action_type FROM action_log_entry WHERE target_id = $1 ORDER BY occurred_at",
    )
    .bind(employment_id.as_str())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        action_types,
        vec!["opening_balance_created", "opening_balance_changed"]
    );
}

// ---- Guard 1: SaltCoverageStart must be a PayPeriod end on the schedule ----

#[sqlx::test]
async fn a_salt_coverage_start_that_is_not_a_period_end_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id) =
        an_employer_and_employment(&db, calendar_month_schedule(), date(2026, 3, 1)).await;

    let result = record_opening_balance(
        &db,
        &employment_id,
        TaxYear::starting(2026),
        date(2026, 9, 15),
        Money::ZERO,
        Money::ZERO,
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::SaltCoverageStartNotAPeriodEnd {
            salt_coverage_start: date(2026, 9, 15),
        })
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM opening_balance")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "a refused boundary must not be written");
}

/// The same guard on a schedule whose period end is not the last day of the
/// month, so a nearby-but-wrong date must still be refused.
#[sqlx::test]
async fn a_salt_coverage_start_one_day_off_the_twenty_sixth_schedule_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id) =
        an_employer_and_employment(&db, twenty_sixth_schedule(), date(2026, 3, 26)).await;

    let result = record_opening_balance(
        &db,
        &employment_id,
        TaxYear::starting(2026),
        date(2026, 9, 26),
        Money::ZERO,
        Money::ZERO,
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::SaltCoverageStartNotAPeriodEnd {
            salt_coverage_start: date(2026, 9, 26),
        })
    );
}

// ---- Guard 2: SaltCoverageStart must fall inside the row's TaxYear ----

#[sqlx::test]
async fn a_salt_coverage_start_outside_the_stated_tax_year_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    // The Employment has existed for years, so guard 3 is satisfied by any
    // date in either TaxYear; only guard 2 can be failing here.
    let (_, employment_id) =
        an_employer_and_employment(&db, calendar_month_schedule(), date(2020, 1, 1)).await;

    let result = record_opening_balance(
        &db,
        &employment_id,
        TaxYear::starting(2025),
        date(2026, 10, 31),
        Money::ZERO,
        Money::ZERO,
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::SaltCoverageStartOutsideTaxYear {
            salt_coverage_start: date(2026, 10, 31),
            tax_year: TaxYear::starting(2025),
        })
    );
}

/// ADR-0005 keys a `PayPeriod`'s `TaxYear` on its **end** date alone, so a
/// period ending in January or February belongs to the TaxYear that started
/// the previous March. Guard 2 must read the boundary that way and not as a
/// bare calendar year, or an Employer adopting Salt in February would be
/// refused for stating the only TaxYear their boundary can be in.
#[sqlx::test]
async fn a_february_boundary_belongs_to_the_tax_year_that_started_the_previous_march(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id) =
        an_employer_and_employment(&db, calendar_month_schedule(), date(2026, 3, 1)).await;

    record_opening_balance(
        &db,
        &employment_id,
        TaxYear::starting(2026),
        date(2027, 2, 28),
        Money::from_cents(1_200_000).unwrap(),
        Money::from_cents(240_000).unwrap(),
        "actor",
    )
    .await
    .unwrap();

    // The same boundary read as its own calendar year is the wrong TaxYear,
    // and is refused.
    let result = record_opening_balance(
        &db,
        &employment_id,
        TaxYear::starting(2027),
        date(2027, 2, 28),
        Money::ZERO,
        Money::ZERO,
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::SaltCoverageStartOutsideTaxYear {
            salt_coverage_start: date(2027, 2, 28),
            tax_year: TaxYear::starting(2027),
        })
    );
}

// ---- Guard 3: on or after the Employment's first payable period end ----

#[sqlx::test]
async fn a_salt_coverage_start_before_the_employments_first_payable_period_is_refused(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool.clone());
    // The Employment starts in June, so May and earlier were never payable
    // for it, even though May is inside the TaxYear.
    let (_, employment_id) =
        an_employer_and_employment(&db, calendar_month_schedule(), date(2026, 6, 1)).await;

    let result = record_opening_balance(
        &db,
        &employment_id,
        TaxYear::starting(2026),
        date(2026, 5, 31),
        Money::ZERO,
        Money::ZERO,
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(
            PayrollAppError::SaltCoverageStartBeforeEmploymentIsPayable {
                salt_coverage_start: date(2026, 5, 31),
                first_payable_period_end: date(2026, 6, 30),
            }
        )
    );
}

/// A continuing employee whose Employment predates the TaxYear entirely: the
/// first payable period is the TaxYear's own first period (March), not the
/// Employment's original start date years earlier. A boundary set at that
/// TaxYear's own first period end is an empty covered span, so zero figures
/// are accepted and non-zero figures are refused, even though the
/// Employment itself is years older.
#[sqlx::test]
async fn a_continuing_employees_first_payable_period_is_the_tax_years_own_first_period(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id) =
        an_employer_and_employment(&db, calendar_month_schedule(), date(2020, 1, 1)).await;

    record_opening_balance(
        &db,
        &employment_id,
        TaxYear::starting(2026),
        date(2026, 3, 31),
        Money::ZERO,
        Money::ZERO,
        "actor",
    )
    .await
    .unwrap();

    let result = record_opening_balance(
        &db,
        &employment_id,
        TaxYear::starting(2026),
        date(2026, 3, 31),
        Money::from_cents(1).unwrap(),
        Money::ZERO,
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(
            PayrollAppError::OpeningBalanceFiguresOverAnEmptyCoveredSpan {
                salt_coverage_start: date(2026, 3, 31),
            }
        )
    );
}

// ---- Guard 4: non-zero figures over an empty covered span ----

#[sqlx::test]
async fn zero_figures_at_the_employments_first_payable_period_are_accepted(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    // The Employment's very first payable period is also the boundary: the
    // covered span is empty, and zero figures over an empty span state
    // nothing false.
    let (_, employment_id) =
        an_employer_and_employment(&db, calendar_month_schedule(), date(2026, 3, 1)).await;

    record_opening_balance(
        &db,
        &employment_id,
        TaxYear::starting(2026),
        date(2026, 3, 31),
        Money::ZERO,
        Money::ZERO,
        "actor",
    )
    .await
    .unwrap();

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM opening_balance")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[sqlx::test]
async fn non_zero_figures_at_the_employments_first_payable_period_are_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id) =
        an_employer_and_employment(&db, calendar_month_schedule(), date(2026, 3, 1)).await;

    let result = record_opening_balance(
        &db,
        &employment_id,
        TaxYear::starting(2026),
        date(2026, 3, 31),
        Money::from_cents(1).unwrap(),
        Money::ZERO,
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(
            PayrollAppError::OpeningBalanceFiguresOverAnEmptyCoveredSpan {
                salt_coverage_start: date(2026, 3, 31),
            }
        )
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM opening_balance")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test]
async fn non_zero_paye_alone_at_an_empty_covered_span_is_also_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id) =
        an_employer_and_employment(&db, calendar_month_schedule(), date(2026, 3, 1)).await;

    let result = record_opening_balance(
        &db,
        &employment_id,
        TaxYear::starting(2026),
        date(2026, 3, 31),
        Money::ZERO,
        Money::from_cents(1).unwrap(),
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(
            PayrollAppError::OpeningBalanceFiguresOverAnEmptyCoveredSpan {
                salt_coverage_start: date(2026, 3, 31),
            }
        )
    );
}

// ---- Employment existence and voiding ----

#[sqlx::test]
async fn recording_against_a_missing_employment_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let missing = EmploymentId::new("does-not-exist");

    let result = record_opening_balance(
        &db,
        &missing,
        TaxYear::starting(2026),
        date(2026, 9, 30),
        Money::ZERO,
        Money::ZERO,
        "actor",
    )
    .await;

    assert_eq!(result, Err(PayrollAppError::EmploymentNotFound(missing)));
}

#[sqlx::test]
async fn a_voided_employment_accepts_no_opening_balance(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id) =
        an_employer_and_employment(&db, calendar_month_schedule(), date(2026, 3, 1)).await;
    void_employment(&db, &employment_id, "actor").await.unwrap();

    let result = record_opening_balance(
        &db,
        &employment_id,
        TaxYear::starting(2026),
        date(2026, 9, 30),
        Money::ZERO,
        Money::ZERO,
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()))
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM opening_balance")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test]
async fn an_unattributed_opening_balance_is_refused_by_the_database(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id) =
        an_employer_and_employment(&db, calendar_month_schedule(), date(2026, 3, 1)).await;

    let result = record_opening_balance(
        &db,
        &employment_id,
        TaxYear::starting(2026),
        date(2026, 9, 30),
        Money::ZERO,
        Money::ZERO,
        "",
    )
    .await;

    assert!(
        matches!(result, Err(PayrollAppError::Database(_))),
        "expected a Database refusal, got {result:?}"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM opening_balance")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

// ---- Never automatic (§4.5, ADR-0014) ----

/// An `OpeningBalance` is an affirmative payroll fact or it is nothing: no
/// other use case may write one, because a Salt-written zero row would turn
/// "nobody entered prior year-to-date" into "confirmed zero" — the exact
/// conflation INV-012 exists to prevent (ADR-0014).
///
/// Every standing-fact use case an Employer reaches before their first
/// payroll runs here, and the table stays empty through all of them.
#[sqlx::test]
async fn no_other_use_case_writes_an_opening_balance(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id) =
        an_employer_and_employment(&db, calendar_month_schedule(), date(2026, 3, 1)).await;

    let count = |pool: PgPool| async move {
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM opening_balance")
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    assert_eq!(
        count(pool.clone()).await,
        0,
        "creating an Employer and an Employment must write no OpeningBalance"
    );

    record_compensation_terms(
        &db,
        &employment_id,
        date(2026, 3, 1),
        Money::from_cents(1_500_000).unwrap(),
        &[],
        "",
        "actor",
    )
    .await
    .unwrap();
    declare_prior_employment(
        &db,
        &employment_id,
        TaxYear::starting(2026),
        PriorEmployment::None,
        "actor",
    )
    .await
    .unwrap();
    declare_unsupported_deduction_status(
        &db,
        &employment_id,
        date(2026, 3, 1),
        UnsupportedDeductionStatus::ConfirmedNone,
        &[],
        "a reason",
        "actor",
    )
    .await
    .unwrap();

    assert_eq!(
        count(pool).await,
        0,
        "no standing-fact use case may write an OpeningBalance; only \
         record_opening_balance does"
    );
}

/// ADR-0014: the boundary is per Employment, not per Employer. One
/// Employer adopting Salt in October has a continuing employee whose
/// pre-Salt figures run March–September, and a November joiner whose
/// covered span is empty. Both rows coexist, each with its own boundary.
#[sqlx::test]
async fn the_boundary_is_per_employment_not_per_employer(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = create_employer(&db, "Employer", calendar_month_schedule(), "actor")
        .await
        .unwrap();
    let (_, continuing) = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::New("person-1".to_string()),
        date(2026, 3, 1),
        None,
        "actor",
    )
    .await
    .unwrap();
    let (_, joiner) = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::New("person-2".to_string()),
        date(2026, 11, 1),
        None,
        "actor",
    )
    .await
    .unwrap();

    record_opening_balance(
        &db,
        &continuing,
        TaxYear::starting(2026),
        date(2026, 10, 31),
        Money::from_cents(700_000).unwrap(),
        Money::from_cents(140_000).unwrap(),
        "actor",
    )
    .await
    .unwrap();
    record_opening_balance(
        &db,
        &joiner,
        TaxYear::starting(2026),
        date(2026, 11, 30),
        Money::ZERO,
        Money::ZERO,
        "actor",
    )
    .await
    .unwrap();

    let boundaries: Vec<(String, NaiveDate, i64)> = sqlx::query_as(
        "SELECT employment_id, first_salt_period_end, prior_taxable_remuneration
         FROM opening_balance ORDER BY first_salt_period_end",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        boundaries,
        vec![
            (continuing.as_str().to_owned(), date(2026, 10, 31), 700_000),
            (joiner.as_str().to_owned(), date(2026, 11, 30), 0),
        ]
    );

    // The joiner's own figures must still be zero: their covered span is
    // empty, whatever the other Employment's boundary says.
    let result = record_opening_balance(
        &db,
        &joiner,
        TaxYear::starting(2026),
        date(2026, 11, 30),
        Money::from_cents(1).unwrap(),
        Money::ZERO,
        "actor",
    )
    .await;
    assert_eq!(
        result,
        Err(
            PayrollAppError::OpeningBalanceFiguresOverAnEmptyCoveredSpan {
                salt_coverage_start: date(2026, 11, 30),
            }
        )
    );
}
