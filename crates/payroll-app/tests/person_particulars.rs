//! Proves the two acceptance criteria `crates/payroll-app/src/person_particulars.rs`'s
//! own unit tests do not: divergence against live finalized payroll (§6.5,
//! generalized the same way issue #71 generalized it for `EmployerParticulars`),
//! and that a `Legacy Person <id>` row the migration 0031 backfill created is
//! correctable exactly like any other Person (issue #72's own acceptance
//! criteria). Both need a real finalized run or a real migration row, which
//! is why they live here rather than beside the use case.
//!
//! Divergence is proved on all four of its faces, matching what
//! `master_data_correction.rs` already asks of an `EmployerParticulars`
//! correction: it is named and demanded, it is recorded in the `ActionLog`
//! once acknowledged, it touches no finalized row, and it counts Live
//! periods only.

use chrono::NaiveDate;
use payroll::{
    EmployerId, EmploymentId, Money, PayPeriod, PeriodEndDay, PersonId, PriorEmployment,
};
use payroll_app::{
    EmploymentPerson, PayrollAppError, SaltDatabase, calculate_payroll_run, create_employer,
    create_employment, create_ordinary_payroll_run, declare_prior_employment,
    declare_unsupported_deduction_status, finalize_payroll_run, get_person_particulars,
    record_compensation_terms, reverse_finalized_payroll, set_person_particulars,
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
) -> (PersonId, EmploymentId) {
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
    (person_id, employment_id)
}

/// An MD5 fingerprint of the whole `finalized_payroll` row for
/// `(employment_id, period_end)`, live or reversed — the same proof
/// `master_data_correction.rs` uses of an `EmployerParticulars` correction,
/// asked of a Person one: a §6.5 warning must leave an already-frozen row
/// byte-identical, and hand-listing every column would only rot.
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
    let (person_id, _employment_id) = a_payable_person(
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
    let (person_id, _employment_id) = a_payable_person(
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
    let (person_id, _employment_id) = a_payable_person(
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

/// Issue #72's divergence criterion applied to a *correction* rather than a
/// first record: the earlier tests prove the first write over live finalized
/// payroll is refused unacknowledged, and this one proves the second write is
/// too — and that the acknowledgement is recorded rather than merely
/// demanded, by naming the periods in the `ActionLog` entry itself. Without
/// this last assertion the acknowledgement would be a gate that leaves no
/// trace of having been passed, which is exactly what §6.5 refuses to allow.
#[sqlx::test]
async fn correcting_existing_particulars_over_live_finalized_payroll_names_the_periods_in_the_log(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let (person_id, _employment_id) = a_payable_person(
        &db,
        &employer_id,
        "Ada Lovelace",
        Money::from_cents(500000).unwrap(),
    )
    .await;
    set_person_particulars(
        &db,
        &employer_id,
        &person_id,
        particulars("12345678901"),
        &[],
        "",
        "actor",
    )
    .await
    .unwrap();
    finalize_period(&db, &employer_id, march()).await;

    let unacknowledged = set_person_particulars(
        &db,
        &employer_id,
        &person_id,
        particulars("98765432109"),
        &[],
        "the identity number was captured from the wrong document",
        "operator:alice",
    )
    .await;
    assert_eq!(
        unacknowledged,
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
        Some("12345678901".to_string()),
        "a refused correction leaves the stored value alone"
    );

    let diverging = set_person_particulars(
        &db,
        &employer_id,
        &person_id,
        particulars("98765432109"),
        &[march()],
        "the identity number was captured from the wrong document",
        "operator:alice",
    )
    .await
    .unwrap();
    assert_eq!(diverging, vec![march()]);

    let logged: (String, serde_json::Value) = sqlx::query_as(
        "SELECT actor, context FROM action_log_entry
         WHERE target_id = $1 AND action_type = 'person_particulars_corrected'",
    )
    .bind(person_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(logged.0, "operator:alice");
    assert_eq!(
        logged.1["reason"],
        "the identity number was captured from the wrong document"
    );
    assert_eq!(logged.1["before"]["identity_number"], "12345678901");
    assert_eq!(logged.1["after"]["identity_number"], "98765432109");
    assert_eq!(
        logged.1["diverging_live_finalized_periods"],
        serde_json::json!([{ "period_start": "2026-03-01", "period_end": "2026-03-31" }]),
        "the acknowledgement is recorded, not merely demanded"
    );
}

/// §6.5's divergence is a warning, never a refusal, and it rewrites nothing
/// it warns about. The `finalized_payroll` REVOKE makes the first half
/// structurally true; this proves the second — that the correction also
/// leaves the row's *liveness* alone, which no grant enforces, so a
/// correction can never quietly cancel a payroll it merely disagrees with.
#[sqlx::test]
async fn an_acknowledged_person_correction_touches_no_finalized_row(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let (person_id, employment_id) = a_payable_person(
        &db,
        &employer_id,
        "Ada Lovelaec",
        Money::from_cents(500000).unwrap(),
    )
    .await;
    finalize_period(&db, &employer_id, march()).await;

    let frozen_before = finalized_payroll_fingerprint(&pool, &employment_id, march().end()).await;
    let live_before =
        live_finalized_payroll_fingerprint(&pool, &employment_id, march().end()).await;

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

    set_person_particulars(
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

    assert_eq!(
        finalized_payroll_fingerprint(&pool, &employment_id, march().end()).await,
        frozen_before,
        "correcting a Person rewrites no finalized row"
    );
    assert_eq!(
        live_finalized_payroll_fingerprint(&pool, &employment_id, march().end()).await,
        live_before,
        "correcting a Person cancels no finalized payroll either"
    );
}

/// The divergence list is Live finalized payroll, not all of it. A reversed
/// period has already been cancelled, so a correction has nothing to diverge
/// from there — and because the acknowledgement must be *exact*, naming it
/// anyway would be refused rather than tolerated.
#[sqlx::test]
async fn a_reversed_period_is_not_named_as_diverging_from_a_person_correction(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let (person_id, employment_id) = a_payable_person(
        &db,
        &employer_id,
        "Ada Lovelaec",
        Money::from_cents(500000).unwrap(),
    )
    .await;
    finalize_period(&db, &employer_id, march()).await;

    let april = PayPeriod::new(date(2026, 4, 1), date(2026, 4, 30)).unwrap();
    let april_original =
        create_ordinary_payroll_run(&db, &employer_id, april, april.end(), "actor")
            .await
            .unwrap();
    assert_eq!(
        calculate_payroll_run(&db, &april_original, "calculator")
            .await
            .unwrap(),
        Vec::new()
    );
    let april_finalized = finalize_payroll_run(&db, &april_original, "finalizer")
        .await
        .unwrap()
        .finalized
        .into_iter()
        .find(|(id, _)| *id == employment_id)
        .expect("April must have finalized for this Employment")
        .1;
    reverse_finalized_payroll(&db, &april_finalized, "April was paid wrong", "actor")
        .await
        .unwrap();

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
}
