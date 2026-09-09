//! Proves the use cases issue #26 introduces: `create_employer`,
//! `create_employment`, `record_compensation_terms`, `void_employment`, and
//! `get_employment_snapshot` — the seams `docs/domain/payroll-run-persistence.md`
//! §4.2-§4.4 and §10 describe, reached through the public API a later ticket
//! calls, not raw SQL.

use chrono::NaiveDate;
use payroll::{DayOfMonth, Money, PayrollError, PeriodEndDay};
use payroll_app::{
    EmploymentPerson, PayrollAppError, SaltDatabase, create_employer, create_employment,
    get_employment_snapshot, record_compensation_terms, void_employment,
};
use sqlx::{PgPool, Row};

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// A 26th-to-25th monthly schedule: 2026-01-26 to 2026-02-25 is one of its
/// own `PayPeriod`s, so `2026-01-26` and `2026-02-26` are period starts and
/// `2026-01-10` is not.
fn twenty_sixth_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap()))
}

#[sqlx::test]
async fn an_employer_is_created_with_exactly_one_pay_schedule(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let schedule = twenty_sixth_schedule();
    let employer_id = create_employer(&db, "Employer", schedule, "actor")
        .await
        .unwrap();

    let row =
        sqlx::query("SELECT period_end_day_kind, period_end_day_value FROM employer WHERE id = $1")
            .bind(employer_id.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.get::<String, _>(0), "day");
    assert_eq!(row.get::<i16, _>(1), 25);
}

#[sqlx::test]
async fn an_employer_is_created_with_its_name(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = create_employer(&db, "Acme Corp", twenty_sixth_schedule(), "actor")
        .await
        .unwrap();

    let name: String = sqlx::query_scalar("SELECT name FROM employer WHERE id = $1")
        .bind(employer_id.as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "Acme Corp");
}

#[sqlx::test]
async fn creating_an_employer_with_a_blank_name_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    // A name of `" "` shows a person nothing while looking like it shows
    // something, and this column is never edited after creation (§0.39).
    for blank in ["", " ", "\t\n  "] {
        let result = create_employer(&db, blank, twenty_sixth_schedule(), "actor").await;

        assert_eq!(
            result,
            Err(PayrollAppError::EmployerNameCannotBeEmpty),
            "a name of {blank:?} must be refused"
        );
    }
}

#[sqlx::test]
async fn an_employment_is_created_with_a_start_date_and_an_optional_end_date(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = create_employer(&db, "Employer", twenty_sixth_schedule(), "actor")
        .await
        .unwrap();
    let (_, employment_id) = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::New("Test Person".to_string()),
        date(2026, 1, 26),
        None,
        "actor",
    )
    .await
    .unwrap();

    let row = sqlx::query("SELECT start_date, end_date, is_void FROM employment WHERE id = $1")
        .bind(employment_id.as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<NaiveDate, _>(0), date(2026, 1, 26));
    assert_eq!(row.get::<Option<NaiveDate>, _>(1), None);
    assert!(!row.get::<bool, _>(2));

    let (_, leaver_id) = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::New("Another Person".to_string()),
        date(2026, 1, 26),
        Some(date(2026, 6, 25)),
        "actor",
    )
    .await
    .unwrap();
    let row = sqlx::query("SELECT end_date FROM employment WHERE id = $1")
        .bind(leaver_id.as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.get::<Option<NaiveDate>, _>(0), Some(date(2026, 6, 25)));
}

async fn an_employer_and_employment(
    db: &SaltDatabase,
) -> (
    payroll::EmployerId,
    payroll::EmploymentId,
    payroll::PersonId,
) {
    let employer_id = create_employer(db, "Employer", twenty_sixth_schedule(), "actor")
        .await
        .unwrap();
    let (person_id, employment_id) = create_employment(
        db,
        &employer_id,
        EmploymentPerson::New("Test Person".to_string()),
        date(2026, 1, 26),
        None,
        "actor",
    )
    .await
    .unwrap();
    (employer_id, employment_id, person_id)
}

#[sqlx::test]
async fn compensation_terms_are_accepted_on_a_pay_periods_own_start_date(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id, _) = an_employer_and_employment(&db).await;

    record_compensation_terms(
        &db,
        &employment_id,
        date(2026, 1, 26),
        Money::from_cents(500000).unwrap(),
        payroll::OrdinaryHours::new(rust_decimal::Decimal::new(4_000, 2)).unwrap(),
        &[],
        "",
        "actor",
    )
    .await
    .unwrap();

    let row = sqlx::query(
        "SELECT effective_from, basic_pay FROM compensation_terms WHERE employment_id = $1",
    )
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<NaiveDate, _>(0), date(2026, 1, 26));
    assert_eq!(row.get::<i64, _>(1), 500000);
}

#[sqlx::test]
async fn an_effective_from_that_is_not_a_pay_period_start_is_a_domain_refusal(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id, _) = an_employer_and_employment(&db).await;

    let result = record_compensation_terms(
        &db,
        &employment_id,
        date(2026, 1, 10),
        Money::from_cents(500000).unwrap(),
        payroll::OrdinaryHours::new(rust_decimal::Decimal::new(4_000, 2)).unwrap(),
        &[],
        "",
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::from(
            PayrollError::EffectiveFromNotAPeriodStart {
                next_valid_effective_from: date(2026, 1, 26),
            }
        ))
    );

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM compensation_terms WHERE employment_id = $1")
            .bind(employment_id.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        count, 0,
        "a refused CompensationTerms row must not be written"
    );
}

#[sqlx::test]
async fn recording_compensation_terms_against_a_missing_employment_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let missing = payroll::EmploymentId::new("does-not-exist");

    let result = record_compensation_terms(
        &db,
        &missing,
        date(2026, 1, 26),
        Money::from_cents(500000).unwrap(),
        payroll::OrdinaryHours::new(rust_decimal::Decimal::new(4_000, 2)).unwrap(),
        &[],
        "",
        "actor",
    )
    .await;

    assert_eq!(result, Err(PayrollAppError::EmploymentNotFound(missing)));
}

/// Issue #69: recording pay from a date this Employment already has a row at
/// is a fact about what the caller asked for, so Rust names it rather than
/// letting `UNIQUE (employment_id, effective_from)` reach the caller as a
/// `Database` refusal — the same refusal `correct_compensation_terms`
/// already raises for the same collision on a move.
#[sqlx::test]
async fn a_duplicate_effective_from_is_a_domain_refusal(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id, _) = an_employer_and_employment(&db).await;
    let basic_pay = Money::from_cents(500000).unwrap();

    record_compensation_terms(
        &db,
        &employment_id,
        date(2026, 1, 26),
        basic_pay,
        payroll::OrdinaryHours::new(rust_decimal::Decimal::new(4_000, 2)).unwrap(),
        &[],
        "",
        "actor",
    )
    .await
    .unwrap();

    let result = record_compensation_terms(
        &db,
        &employment_id,
        date(2026, 1, 26),
        basic_pay,
        payroll::OrdinaryHours::new(rust_decimal::Decimal::new(4_000, 2)).unwrap(),
        &[],
        "",
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::CompensationTermsAlreadyExistAt {
            employment_id: employment_id.clone(),
            effective_from: date(2026, 1, 26),
        })
    );

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM compensation_terms WHERE employment_id = $1")
            .bind(employment_id.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "the first row stands, unchanged and alone");
}

#[sqlx::test]
async fn an_employment_can_be_voided_and_is_never_physically_deleted(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id, _) = an_employer_and_employment(&db).await;

    void_employment(&db, &employment_id, "actor").await.unwrap();

    let row = sqlx::query("SELECT is_void FROM employment WHERE id = $1")
        .bind(employment_id.as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(row.get::<bool, _>(0), "the Employment must be marked void");
}

#[sqlx::test]
async fn voiding_writes_an_employment_voided_action_log_entry(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (employer_id, employment_id, _) = an_employer_and_employment(&db).await;

    void_employment(&db, &employment_id, "actor").await.unwrap();

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
    assert_eq!(row.get::<String, _>(2), "employment_voided");
    assert_eq!(row.get::<String, _>(3), "employment");
}

#[sqlx::test]
async fn voiding_a_missing_employment_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let missing = payroll::EmploymentId::new("does-not-exist");

    let result = void_employment(&db, &missing, "actor").await;

    assert_eq!(result, Err(PayrollAppError::EmploymentNotFound(missing)));

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM action_log_entry")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "a refused void must write no ActionLog entry");
}

#[sqlx::test]
async fn reading_an_employment_back_yields_a_snapshot_the_pure_crate_accepts(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (employer_id, employment_id, person_id) = an_employer_and_employment(&db).await;
    let basic_pay = Money::from_cents(500000).unwrap();
    record_compensation_terms(
        &db,
        &employment_id,
        date(2026, 1, 26),
        basic_pay,
        payroll::OrdinaryHours::new(rust_decimal::Decimal::new(4_000, 2)).unwrap(),
        &[],
        "",
        "actor",
    )
    .await
    .unwrap();

    let snapshot = get_employment_snapshot(&db, &employment_id, date(2026, 2, 1))
        .await
        .unwrap();

    assert_eq!(snapshot.employment_id(), &employment_id);
    assert_eq!(snapshot.employer_id(), &employer_id);
    assert_eq!(snapshot.person().person_id().as_str(), person_id.as_str());
    assert_eq!(snapshot.start_date(), date(2026, 1, 26));
    assert_eq!(snapshot.end_date(), None);
    assert_eq!(
        snapshot.compensation_terms().effective_from(),
        date(2026, 1, 26)
    );
    assert_eq!(snapshot.compensation_terms().effective_until(), None);
    assert_eq!(snapshot.compensation_terms().basic_pay(), basic_pay);
}

/// §4.4: a `CompensationTerms` row is in force until the next row's
/// `effective_from`. `get_employment_snapshot` derives that boundary itself
/// — there is no `effective_until` column to read it from.
#[sqlx::test]
async fn a_compensation_terms_row_stays_in_force_until_the_next_rows_effective_from(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id, _) = an_employer_and_employment(&db).await;
    let march_pay = Money::from_cents(500000).unwrap();
    let april_pay = Money::from_cents(550000).unwrap();
    record_compensation_terms(
        &db,
        &employment_id,
        date(2026, 1, 26),
        march_pay,
        payroll::OrdinaryHours::new(rust_decimal::Decimal::new(4_000, 2)).unwrap(),
        &[],
        "",
        "actor",
    )
    .await
    .unwrap();
    record_compensation_terms(
        &db,
        &employment_id,
        date(2026, 2, 26),
        april_pay,
        payroll::OrdinaryHours::new(rust_decimal::Decimal::new(4_000, 2)).unwrap(),
        &[],
        "",
        "actor",
    )
    .await
    .unwrap();

    let first_period = get_employment_snapshot(&db, &employment_id, date(2026, 2, 1))
        .await
        .unwrap();
    assert_eq!(first_period.compensation_terms().basic_pay(), march_pay);
    assert_eq!(
        first_period.compensation_terms().effective_until(),
        Some(date(2026, 2, 25)),
        "the first row must end the day before the second row begins"
    );

    let second_period = get_employment_snapshot(&db, &employment_id, date(2026, 3, 1))
        .await
        .unwrap();
    assert_eq!(second_period.compensation_terms().basic_pay(), april_pay);
    assert_eq!(second_period.compensation_terms().effective_until(), None);
}

#[sqlx::test]
async fn reading_an_employment_with_no_compensation_terms_in_force_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id, _) = an_employer_and_employment(&db).await;

    let result = get_employment_snapshot(&db, &employment_id, date(2026, 2, 1)).await;

    assert_eq!(
        result,
        Err(PayrollAppError::NoCompensationTermsInForce(employment_id))
    );
}

#[sqlx::test]
async fn reading_a_missing_employment_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let missing = payroll::EmploymentId::new("does-not-exist");

    let result = get_employment_snapshot(&db, &missing, date(2026, 2, 1)).await;

    assert_eq!(result, Err(PayrollAppError::EmploymentNotFound(missing)));
}

/// §4.3 keeps a voided Employment out of every payroll, so the calculation
/// input read for one is refused rather than handed to the pure crate.
#[sqlx::test]
async fn a_voided_employment_yields_no_snapshot_to_calculate_from(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id, _) = an_employer_and_employment(&db).await;
    record_compensation_terms(
        &db,
        &employment_id,
        date(2026, 1, 26),
        Money::from_cents(500000).unwrap(),
        payroll::OrdinaryHours::new(rust_decimal::Decimal::new(4_000, 2)).unwrap(),
        &[],
        "",
        "actor",
    )
    .await
    .unwrap();
    void_employment(&db, &employment_id, "actor").await.unwrap();

    let result = get_employment_snapshot(&db, &employment_id, date(2026, 2, 1)).await;

    assert_eq!(
        result,
        Err(PayrollAppError::EmploymentIsVoid(employment_id))
    );
}

#[sqlx::test]
async fn a_voided_employment_accepts_no_further_compensation_terms(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id, _) = an_employer_and_employment(&db).await;
    void_employment(&db, &employment_id, "actor").await.unwrap();

    let result = record_compensation_terms(
        &db,
        &employment_id,
        date(2026, 1, 26),
        Money::from_cents(500000).unwrap(),
        payroll::OrdinaryHours::new(rust_decimal::Decimal::new(4_000, 2)).unwrap(),
        &[],
        "",
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()))
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM compensation_terms WHERE employment_id = $1")
            .bind(employment_id.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
}

/// The void already happened. A second `EmploymentVoided` entry would record
/// an act that did not, in a log no role may afterwards correct.
#[sqlx::test]
async fn voiding_an_already_void_employment_writes_no_second_action_log_entry(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id, _) = an_employer_and_employment(&db).await;
    void_employment(&db, &employment_id, "actor").await.unwrap();

    let result = void_employment(&db, &employment_id, "actor").await;

    assert_eq!(
        result,
        Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()))
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM action_log_entry WHERE target_id = $1")
            .bind(employment_id.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "one void is one ActionLog entry");
}

#[sqlx::test]
async fn an_employment_that_ends_before_it_starts_is_a_domain_refusal(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = create_employer(&db, "Employer", twenty_sixth_schedule(), "actor")
        .await
        .unwrap();

    let result = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::New("Test Person".to_string()),
        date(2026, 6, 26),
        Some(date(2026, 1, 25)),
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::EmploymentEndsBeforeItStarts {
            start_date: date(2026, 6, 26),
            end_date: date(2026, 1, 25),
        })
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM employment")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test]
async fn an_employment_against_a_missing_employer_is_a_domain_refusal(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let missing = payroll::EmployerId::new("does-not-exist");

    let result = create_employment(
        &db,
        &missing,
        EmploymentPerson::New("Test Person".to_string()),
        date(2026, 1, 26),
        None,
        "actor",
    )
    .await;

    assert_eq!(result, Err(PayrollAppError::EmployerNotFound(missing)));
}

/// §10 makes the ActionLog "who did what and when", and no role may UPDATE
/// it afterwards, so a blank actor is an unfixable row that answers "who"
/// with nothing.
#[sqlx::test]
async fn an_unattributed_void_is_refused_by_the_database(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id, _) = an_employer_and_employment(&db).await;

    let result = void_employment(&db, &employment_id, "").await;

    assert!(
        matches!(result, Err(PayrollAppError::Database(_))),
        "expected a Database refusal, got {result:?}"
    );
    let row = sqlx::query("SELECT is_void FROM employment WHERE id = $1")
        .bind(employment_id.as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        !row.get::<bool, _>(0),
        "a refused void must leave the Employment untouched"
    );
}

/// `get_employment_snapshot` rebuilds a `Money` from this column and can
/// only `expect` that to succeed, so the column itself refuses the amount
/// that would break it.
#[sqlx::test]
async fn a_negative_basic_pay_is_refused_by_the_database(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, employment_id, _) = an_employer_and_employment(&db).await;

    let result = sqlx::query(
        "INSERT INTO compensation_terms (employment_id, effective_from, basic_pay, created_by)
         VALUES ($1, '2026-01-26', -1, 'actor')",
    )
    .bind(employment_id.as_str())
    .execute(&pool)
    .await;

    assert!(
        result.is_err(),
        "basic_pay is a Money amount, never negative"
    );
}
