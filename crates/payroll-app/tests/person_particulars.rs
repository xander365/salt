//! Proves the two acceptance criteria `crates/payroll-app/src/person_particulars.rs`'s
//! own unit tests do not: divergence against live finalized payroll (§6.5,
//! generalized the same way issue #71 generalized it for `EmployerParticulars`),
//! and that a `Legacy Person <id>` row the migration 0031 backfill created is
//! correctable exactly like any other Person (issue #72's own acceptance
//! criteria).

use chrono::NaiveDate;
use payroll::{EmployerId, Money, PayPeriod, PeriodEndDay, PersonId, PriorEmployment};
use payroll_app::{
    EmploymentPerson, PayrollAppError, SaltDatabase, calculate_payroll_run, create_employer,
    create_employment, create_ordinary_payroll_run, declare_prior_employment,
    declare_unsupported_deduction_status, finalize_payroll_run, get_person_particulars,
    record_compensation_terms, set_person_particulars,
};
use payroll_app::{PersonParticularsFields, correct_person_full_name};
use sqlx::PgPool;

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn monthly_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::LastDayOfMonth)
}

fn march() -> PayPeriod {
    PayPeriod::new(date(2026, 3, 1), date(2026, 3, 31)).unwrap()
}

async fn an_employer(db: &SaltDatabase) -> EmployerId {
    create_employer(db, "Employer", monthly_schedule(), "actor")
        .await
        .unwrap()
}

fn particulars(identity_number: &str) -> PersonParticularsFields {
    PersonParticularsFields {
        identity_number: identity_number.to_string(),
        address_line1: "1 Independence Ave".to_string(),
        address_line2: None,
        city: "Windhoek".to_string(),
        postal_code: None,
    }
}

/// An Employment starting on March's own first day, with a single
/// `CompensationTerms` row and everything else `calculate` needs, returning
/// the Person it was created for.
async fn a_payable_person(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    full_name: &str,
    basic_pay: Money,
) -> PersonId {
    let (person_id, employment_id) = create_employment(
        db,
        employer_id,
        EmploymentPerson::New(full_name.to_string()),
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
        payroll::TaxYear::for_period_end(march().end()),
        PriorEmployment::None,
        "actor",
    )
    .await
    .unwrap();
    declare_unsupported_deduction_status(
        db,
        &employment_id,
        march().start(),
        payroll::UnsupportedDeductionStatus::ConfirmedNone,
        &[],
        "no unsupported deductions",
        "actor",
    )
    .await
    .unwrap();
    person_id
}

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

#[sqlx::test]
async fn an_unacknowledged_first_record_over_live_finalized_payroll_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let person_id = a_payable_person(
        &db,
        &employer_id,
        "Ada Lovelace",
        Money::from_cents(500000).unwrap(),
    )
    .await;
    finalize_period(&db, &employer_id, march()).await;

    let result = set_person_particulars(
        &db,
        &employer_id,
        &person_id,
        particulars("12345678901"),
        &[],
        "",
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::PersonMasterDataDivergenceNotAcknowledged {
            person_id: person_id.clone(),
            diverging_periods: vec![march()],
        })
    );
    assert_eq!(
        get_person_particulars(&db, &employer_id, &person_id)
            .await
            .unwrap()
            .identity_number,
        None,
        "a refused write records nothing"
    );
}

#[sqlx::test]
async fn an_acknowledged_reasoned_first_record_over_live_finalized_payroll_is_written(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let person_id = a_payable_person(
        &db,
        &employer_id,
        "Ada Lovelace",
        Money::from_cents(500000).unwrap(),
    )
    .await;
    finalize_period(&db, &employer_id, march()).await;

    let diverging = set_person_particulars(
        &db,
        &employer_id,
        &person_id,
        particulars("12345678901"),
        &[march()],
        "particulars recorded after the first payroll already ran",
        "actor",
    )
    .await
    .unwrap();

    assert_eq!(diverging, vec![march()]);
    assert_eq!(
        get_person_particulars(&db, &employer_id, &person_id)
            .await
            .unwrap()
            .identity_number,
        Some("12345678901".to_string())
    );
}

#[sqlx::test]
async fn a_full_name_correction_over_live_finalized_payroll_names_it_and_requires_acknowledgement(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let person_id = a_payable_person(
        &db,
        &employer_id,
        "Ada Lovelaec",
        Money::from_cents(500000).unwrap(),
    )
    .await;
    finalize_period(&db, &employer_id, march()).await;

    let unacknowledged = correct_person_full_name(
        &db,
        &employer_id,
        &person_id,
        "Ada Lovelace",
        &[],
        "fixing a misspelling",
        "actor",
    )
    .await;
    assert_eq!(
        unacknowledged,
        Err(PayrollAppError::PersonMasterDataDivergenceNotAcknowledged {
            person_id: person_id.clone(),
            diverging_periods: vec![march()],
        })
    );

    let diverging = correct_person_full_name(
        &db,
        &employer_id,
        &person_id,
        "Ada Lovelace",
        &[march()],
        "fixing a misspelling",
        "actor",
    )
    .await
    .unwrap();

    assert_eq!(diverging, vec![march()]);
    assert_eq!(
        get_person_particulars(&db, &employer_id, &person_id)
            .await
            .unwrap()
            .full_name,
        "Ada Lovelace"
    );
}

/// Issue #72's own acceptance criterion: a `Legacy Person <id>` row the
/// migration 0031 backfill created — for an Employment that predates
/// `person` entirely — is correctable exactly like any Person created
/// through `create_employment`.
#[sqlx::test]
async fn a_legacy_person_row_is_correctable(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());

    sqlx::query(
        "INSERT INTO employer (id, name, period_end_day_kind, period_end_day_value, created_by)
         VALUES ('legacy-employer', 'Legacy Employer', 'last_day_of_month', NULL, 'test-setup')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO person (id, employer_id, full_name, created_by)
         VALUES ('legacy-person-abc123', 'legacy-employer', 'Legacy Person old-id', 'migration')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let employer_id = EmployerId::new("legacy-employer");
    let person_id = PersonId::new("legacy-person-abc123");

    let diverging = correct_person_full_name(
        &db,
        &employer_id,
        &person_id,
        "Grace Hopper",
        &[],
        "identified the legacy record from payroll history",
        "actor",
    )
    .await
    .unwrap();
    assert_eq!(diverging, Vec::new());

    let diverging = set_person_particulars(
        &db,
        &employer_id,
        &person_id,
        particulars("98765432109"),
        &[],
        "",
        "actor",
    )
    .await
    .unwrap();
    assert_eq!(diverging, Vec::new());

    let stored = get_person_particulars(&db, &employer_id, &person_id)
        .await
        .unwrap();
    assert_eq!(stored.full_name, "Grace Hopper");
    assert_eq!(stored.identity_number, Some("98765432109".to_string()));
}
