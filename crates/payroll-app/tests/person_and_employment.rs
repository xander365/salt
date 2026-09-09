//! Proves the use cases issue #51 introduces: `create_employment` accepting
//! either an existing Person or a full name for a brand-new one, and the two
//! Employer-scoped read models, `list_employments_for_employer` and
//! `get_employment_detail`.

use chrono::NaiveDate;
use payroll::{DayOfMonth, Money, OrdinaryHours, PeriodEndDay, PersonId};
use payroll_app::{
    EmploymentPerson, PayrollAppError, SaltDatabase, create_employer, create_employment,
    get_employment_detail, list_employments_for_employer, record_compensation_terms,
    record_compensation_terms_with_ordinary_hours,
};
use rust_decimal::Decimal;
use sqlx::{PgPool, Row};

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn twenty_sixth_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap()))
}

async fn an_employer(db: &SaltDatabase) -> payroll::EmployerId {
    create_employer(db, "Employer", twenty_sixth_schedule(), "actor")
        .await
        .unwrap()
}

#[sqlx::test]
async fn creating_by_full_name_writes_the_person_and_the_employment_together(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;

    let (person_id, employment_id) = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::New("Ada Lovelace".to_string()),
        date(2026, 1, 26),
        None,
        "actor",
    )
    .await
    .unwrap();

    let person_row = sqlx::query("SELECT employer_id, full_name FROM person WHERE id = $1")
        .bind(person_id.as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(person_row.get::<String, _>(0), employer_id.as_str());
    assert_eq!(person_row.get::<String, _>(1), "Ada Lovelace");

    let employment_row = sqlx::query("SELECT employer_id, person_id FROM employment WHERE id = $1")
        .bind(employment_id.as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(employment_row.get::<String, _>(0), employer_id.as_str());
    assert_eq!(employment_row.get::<String, _>(1), person_id.as_str());
}

#[sqlx::test]
async fn creating_by_an_existing_person_id_reuses_that_person(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let (person_id, _first_employment) = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::New("Ada Lovelace".to_string()),
        date(2026, 1, 26),
        Some(date(2026, 6, 25)),
        "actor",
    )
    .await
    .unwrap();

    let (reused_person_id, second_employment) = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::Existing(person_id.clone()),
        date(2026, 7, 26),
        None,
        "actor",
    )
    .await
    .unwrap();

    assert_eq!(reused_person_id, person_id);

    let person_count: i64 = sqlx::query_scalar("SELECT count(*) FROM person")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(person_count, 1, "no second Person row was created");

    let employment_count: i64 = sqlx::query_scalar("SELECT count(*) FROM employment")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(employment_count, 2);

    let stored_person: String =
        sqlx::query_scalar("SELECT person_id FROM employment WHERE id = $1")
            .bind(second_employment.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stored_person, person_id.as_str());
}

#[sqlx::test]
async fn a_person_id_belonging_to_another_employer_is_refused_as_not_found(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let owning_employer = an_employer(&db).await;
    let other_employer = an_employer(&db).await;
    let (person_id, _) = create_employment(
        &db,
        &owning_employer,
        EmploymentPerson::New("Ada Lovelace".to_string()),
        date(2026, 1, 26),
        None,
        "actor",
    )
    .await
    .unwrap();

    let result = create_employment(
        &db,
        &other_employer,
        EmploymentPerson::Existing(person_id.clone()),
        date(2026, 1, 26),
        None,
        "actor",
    )
    .await;

    assert_eq!(result, Err(PayrollAppError::PersonNotFound(person_id)));

    let employment_count: i64 = sqlx::query_scalar("SELECT count(*) FROM employment")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        employment_count, 1,
        "the refused Employment must not be written"
    );
}

#[sqlx::test]
async fn an_unknown_person_id_is_refused_identically_to_a_cross_employer_one(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let unknown = PersonId::new("does-not-exist");

    let result = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::Existing(unknown.clone()),
        date(2026, 1, 26),
        None,
        "actor",
    )
    .await;

    assert_eq!(result, Err(PayrollAppError::PersonNotFound(unknown)));
}

#[sqlx::test]
async fn a_blank_full_name_is_refused_and_writes_neither_row(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;

    for blank in ["", " ", "\t\n  "] {
        let result = create_employment(
            &db,
            &employer_id,
            EmploymentPerson::New(blank.to_string()),
            date(2026, 1, 26),
            None,
            "actor",
        )
        .await;

        assert_eq!(
            result,
            Err(PayrollAppError::PersonFullNameCannotBeEmpty),
            "a full name of {blank:?} must be refused"
        );
    }

    let person_count: i64 = sqlx::query_scalar("SELECT count(*) FROM person")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(person_count, 0);
    let employment_count: i64 = sqlx::query_scalar("SELECT count(*) FROM employment")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(employment_count, 0);
}

#[sqlx::test]
async fn creating_against_a_missing_employer_writes_neither_row(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let missing = payroll::EmployerId::new("does-not-exist");

    let result = create_employment(
        &db,
        &missing,
        EmploymentPerson::New("Ada Lovelace".to_string()),
        date(2026, 1, 26),
        None,
        "actor",
    )
    .await;

    assert_eq!(result, Err(PayrollAppError::EmployerNotFound(missing)));

    let person_count: i64 = sqlx::query_scalar("SELECT count(*) FROM person")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(person_count, 0);
}

#[sqlx::test]
async fn listing_names_every_employment_with_its_persons_full_name(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let other_employer = an_employer(&db).await;

    let (ada_id, ada_employment) = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::New("Ada Lovelace".to_string()),
        date(2026, 1, 26),
        None,
        "actor",
    )
    .await
    .unwrap();
    let (_, grace_employment) = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::New("Grace Hopper".to_string()),
        date(2026, 1, 26),
        None,
        "actor",
    )
    .await
    .unwrap();
    // Belongs to a different Employer; must never appear in this list.
    create_employment(
        &db,
        &other_employer,
        EmploymentPerson::New("Someone Else".to_string()),
        date(2026, 1, 26),
        None,
        "actor",
    )
    .await
    .unwrap();

    let listings = list_employments_for_employer(&db, &employer_id)
        .await
        .unwrap();

    assert_eq!(listings.len(), 2);
    let ada = listings
        .iter()
        .find(|listing| listing.id == ada_employment)
        .expect("Ada's Employment is listed");
    assert_eq!(ada.person_id, ada_id);
    assert_eq!(ada.full_name, "Ada Lovelace");
    assert!(
        listings
            .iter()
            .any(|listing| listing.id == grace_employment && listing.full_name == "Grace Hopper")
    );
}

#[sqlx::test]
async fn detail_reads_dates_the_persons_full_name_and_current_pay(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let (_, employment_id) = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::New("Ada Lovelace".to_string()),
        date(2026, 1, 26),
        Some(date(2026, 6, 25)),
        "actor",
    )
    .await
    .unwrap();
    let basic_pay = Money::from_cents(500_000).unwrap();
    record_compensation_terms(
        &db,
        &employment_id,
        date(2026, 1, 26),
        basic_pay,
        &[],
        "",
        "actor",
    )
    .await
    .unwrap();

    let detail = get_employment_detail(&db, &employer_id, &employment_id, date(2026, 2, 1))
        .await
        .unwrap();

    assert_eq!(detail.full_name, "Ada Lovelace");
    assert_eq!(detail.start_date, date(2026, 1, 26));
    assert_eq!(detail.end_date, Some(date(2026, 6, 25)));
    assert_eq!(detail.current_basic_pay, Some(basic_pay));
}

#[sqlx::test]
async fn detail_keeps_the_ordinary_hours_recorded_beside_pay_after_a_reload(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let (_, employment_id) = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::New("Ada Lovelace".to_string()),
        date(2026, 1, 26),
        None,
        "actor",
    )
    .await
    .unwrap();

    record_compensation_terms_with_ordinary_hours(
        &db,
        &employment_id,
        date(2026, 1, 26),
        Money::from_cents(500_000).unwrap(),
        Some(OrdinaryHours::new(Decimal::new(4_050, 2)).unwrap()),
        &[],
        "",
        "actor",
    )
    .await
    .unwrap();

    let detail = get_employment_detail(&db, &employer_id, &employment_id, date(2026, 2, 1))
        .await
        .unwrap();
    assert_eq!(
        detail.current_ordinary_hours.map(OrdinaryHours::as_decimal),
        Some(Decimal::new(4_050, 2))
    );
}

#[sqlx::test]
async fn detail_shows_no_current_pay_when_no_compensation_terms_are_in_force(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let (_, employment_id) = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::New("Ada Lovelace".to_string()),
        date(2026, 1, 26),
        None,
        "actor",
    )
    .await
    .unwrap();

    let detail = get_employment_detail(&db, &employer_id, &employment_id, date(2026, 2, 1))
        .await
        .unwrap();

    assert_eq!(detail.current_basic_pay, None);
}

#[sqlx::test]
async fn an_employment_id_belonging_to_another_employer_is_not_found_on_the_list_scope(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool.clone());
    let owning_employer = an_employer(&db).await;
    let other_employer = an_employer(&db).await;
    let (_, employment_id) = create_employment(
        &db,
        &owning_employer,
        EmploymentPerson::New("Ada Lovelace".to_string()),
        date(2026, 1, 26),
        None,
        "actor",
    )
    .await
    .unwrap();

    let result =
        get_employment_detail(&db, &other_employer, &employment_id, date(2026, 2, 1)).await;

    assert_eq!(
        result,
        Err(PayrollAppError::EmploymentNotFound(employment_id))
    );
}

#[sqlx::test]
async fn an_unknown_employment_id_is_not_found_identically(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let missing = payroll::EmploymentId::new("does-not-exist");

    let result = get_employment_detail(&db, &employer_id, &missing, date(2026, 2, 1)).await;

    assert_eq!(result, Err(PayrollAppError::EmploymentNotFound(missing)));
}

/// The `person.full_name` column is append-only (migration 0031 revokes
/// UPDATE), so whitespace a form padded a name with would be unfixable for
/// the life of the row. It is trimmed on the way in instead.
#[sqlx::test]
async fn a_padded_full_name_is_stored_trimmed(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;

    let (_, employment_id) = create_employment(
        &db,
        &employer_id,
        EmploymentPerson::New("  Ada Lovelace\t".to_string()),
        date(2026, 1, 26),
        None,
        "actor",
    )
    .await
    .unwrap();

    let detail = get_employment_detail(&db, &employer_id, &employment_id, date(2026, 2, 1))
        .await
        .unwrap();

    assert_eq!(detail.full_name, "Ada Lovelace");
}
