//! Proves issue #73 (parent #70 D-7; ADR-0004 applied, not amended — D28):
//! finalizing freezes `EmployerParticulars`, `PersonParticulars` and
//! `PayslipTemplateVersion` onto each `FinalizedPayroll`, in the same
//! transaction as everything else finalization writes; correcting either
//! particular afterward leaves the frozen values unchanged; and a payroll
//! finalized before this ticket still reads back, visibly carrying no frozen
//! particulars.
//!
//! Every fixture reaches a real `FinalizedPayroll` through the public API,
//! exactly as `tests/finalize_payroll_run.rs` does — except the one "as an
//! older release would have left it" row, which no public use case can
//! produce (the documented exception `tests/finalized_payroll_read_models.rs`
//! already uses for the same reason).

use chrono::NaiveDate;
use payroll::{
    EmployerId, EmploymentId, Money, PayPeriod, PeriodEndDay, PriorEmployment, TaxYear,
    UnsupportedDeductionStatus,
};
use payroll_app::{
    EmployerParticularsFields, EmploymentPerson, PAYSLIP_TEMPLATE_VERSION, PayrollRunId,
    PersonParticularsFields, SaltDatabase, calculate_payroll_run, correct_person_full_name,
    create_employer, create_employment, create_ordinary_payroll_run, declare_prior_employment,
    declare_unsupported_deduction_status, finalize_payroll_run, get_finalized_payroll_detail,
    record_compensation_terms, set_employer_particulars, set_person_particulars,
};
use sqlx::PgPool;

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn monthly_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::LastDayOfMonth)
}

/// March 2026 — the calendar month `monthly_schedule()` generates. Every
/// Employment below starts on this period's own first day, so no
/// `OpeningBalance` or earlier resolved period is required (§7.1 branch 1).
fn period() -> PayPeriod {
    PayPeriod::new(date(2026, 3, 1), date(2026, 3, 31)).unwrap()
}

async fn an_employer(db: &SaltDatabase) -> EmployerId {
    create_employer(db, "Employer", monthly_schedule(), "actor")
        .await
        .unwrap()
}

fn employer_particulars_fields(registered_name: &str) -> EmployerParticularsFields {
    EmployerParticularsFields {
        registered_name: registered_name.to_string(),
        address_line1: "1 Independence Ave".to_string(),
        address_line2: None,
        city: "Windhoek".to_string(),
        postal_code: Some("10001".to_string()),
        income_tax_number: Some("12345678".to_string()),
        social_security_number: None,
    }
}

fn person_particulars_fields(identity_number: &str) -> PersonParticularsFields {
    PersonParticularsFields {
        identity_number: identity_number.to_string(),
        address_line1: "2 Fidel Castro St".to_string(),
        address_line2: None,
        city: "Swakopmund".to_string(),
        postal_code: Some("9000".to_string()),
    }
}

/// An Employment starting on `period()`'s own first day, with every fact
/// `calculate` needs already on record — the same minimal fixture
/// `tests/finalize_payroll_run.rs` uses. Returns both ids: the Person's, for
/// `set_person_particulars`/`correct_person_full_name`, and the
/// Employment's, for everything else.
async fn a_fully_declared_employment(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    person: &str,
) -> (payroll::PersonId, EmploymentId) {
    let (person_id, employment_id) = create_employment(
        db,
        employer_id,
        EmploymentPerson::New(person.to_string()),
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
        Money::from_cents(1500000).unwrap(),
        payroll::OrdinaryHours::new(rust_decimal::Decimal::new(4_000, 2)).unwrap(),
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
    (person_id, employment_id)
}

/// Creates a March Ordinary run, calculates and finalizes it, and returns
/// the one member's `FinalizedPayrollId` (as text — the public API only
/// ever returns it wrapped, and every assertion here reads it back through
/// `get_finalized_payroll_detail`, which takes the wire string form).
async fn finalize_march(db: &SaltDatabase, employer_id: &EmployerId) -> (PayrollRunId, String) {
    let run_id = create_ordinary_payroll_run(db, employer_id, period(), date(2026, 4, 5), "actor")
        .await
        .unwrap();
    let refusals = calculate_payroll_run(db, &run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(refusals, Vec::new(), "the run must reach Calculated");
    let outcome = finalize_payroll_run(db, &run_id, "finalizer")
        .await
        .unwrap();
    let (_employment_id, finalized_payroll_id) = outcome
        .finalized
        .into_iter()
        .next()
        .expect("an Ordinary run with one member finalizes with one FinalizedPayroll");
    (run_id, finalized_payroll_id.to_string())
}

#[sqlx::test]
async fn finalizing_freezes_the_recorded_particulars_and_the_template_version(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    let (person_id, _employment_id) =
        a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;

    set_employer_particulars(
        &db,
        &employer_id,
        employer_particulars_fields("Acme Corp (Pty) Ltd"),
        &[],
        "",
        "operator:alice",
    )
    .await
    .unwrap();
    set_person_particulars(
        &db,
        &employer_id,
        &person_id,
        person_particulars_fields("80012345678"),
        &[],
        "",
        "operator:alice",
    )
    .await
    .unwrap();

    let (_run_id, finalized_payroll_id) = finalize_march(&db, &employer_id).await;

    let detail = get_finalized_payroll_detail(&db, &employer_id, &finalized_payroll_id)
        .await
        .unwrap();

    let employer_particulars = detail
        .employer_particulars
        .expect("the recorded EmployerParticulars must have frozen");
    assert_eq!(employer_particulars.registered_name, "Acme Corp (Pty) Ltd");
    assert_eq!(employer_particulars.city, "Windhoek");

    let person_particulars = detail
        .person_particulars
        .expect("the recorded PersonParticulars must have frozen");
    assert_eq!(person_particulars.full_name, "Ada Lovelace");
    assert_eq!(
        person_particulars.identity_number.as_deref(),
        Some("80012345678")
    );
    assert_eq!(person_particulars.city.as_deref(), Some("Swakopmund"));

    assert_eq!(detail.full_name, "Ada Lovelace");
    assert_eq!(
        detail.payslip_template_version.as_deref(),
        Some(PAYSLIP_TEMPLATE_VERSION)
    );
}

/// Nothing true to freeze (issue #73's own Deep Instructions) when neither
/// particular was ever recorded: `employer_particulars` reads back `None`,
/// and `person_particulars` still freezes — it always has a `full_name` —
/// with every other field absent. `PayslipTemplateVersion` freezes
/// regardless: it names code, not master data, so it never has "nothing to
/// freeze".
#[sqlx::test]
async fn finalizing_with_no_particulars_ever_recorded_still_freezes_the_template_version(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;

    let (_run_id, finalized_payroll_id) = finalize_march(&db, &employer_id).await;

    let detail = get_finalized_payroll_detail(&db, &employer_id, &finalized_payroll_id)
        .await
        .unwrap();

    assert_eq!(detail.employer_particulars, None);
    let person_particulars = detail
        .person_particulars
        .expect("full_name always freezes, even with nothing else recorded");
    assert_eq!(person_particulars.full_name, "Ada Lovelace");
    assert_eq!(person_particulars.identity_number, None);
    assert_eq!(person_particulars.address_line1, None);
    assert_eq!(
        detail.payslip_template_version.as_deref(),
        Some(PAYSLIP_TEMPLATE_VERSION)
    );
}

/// D22, the acceptance criterion at the heart of issue #73: a correction
/// made after finalization can never rewrite what a `FinalizedPayroll`
/// already froze.
#[sqlx::test]
async fn correcting_particulars_after_finalization_leaves_the_frozen_values_unchanged(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    let (person_id, _employment_id) =
        a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;

    set_employer_particulars(
        &db,
        &employer_id,
        employer_particulars_fields("Acme Corp (Pty) Ltd"),
        &[],
        "",
        "operator:alice",
    )
    .await
    .unwrap();
    set_person_particulars(
        &db,
        &employer_id,
        &person_id,
        person_particulars_fields("80012345678"),
        &[],
        "",
        "operator:alice",
    )
    .await
    .unwrap();

    let (_run_id, finalized_payroll_id) = finalize_march(&db, &employer_id).await;

    // Corrections made after finalization: an acknowledged divergence,
    // since this payroll is now Live and finalized for a period each of
    // these three facts covers.
    let diverging = vec![period()];
    set_employer_particulars(
        &db,
        &employer_id,
        employer_particulars_fields("Acme Holdings"),
        &diverging,
        "registered new legal name",
        "operator:alice",
    )
    .await
    .unwrap();
    set_person_particulars(
        &db,
        &employer_id,
        &person_id,
        person_particulars_fields("90099999999"),
        &diverging,
        "identity number was mistyped",
        "operator:alice",
    )
    .await
    .unwrap();
    correct_person_full_name(
        &db,
        &employer_id,
        &person_id,
        "Ada King, Countess of Lovelace",
        &diverging,
        "full legal name recorded",
        "operator:alice",
    )
    .await
    .unwrap();

    let detail = get_finalized_payroll_detail(&db, &employer_id, &finalized_payroll_id)
        .await
        .unwrap();

    let employer_particulars = detail.employer_particulars.unwrap();
    assert_eq!(employer_particulars.registered_name, "Acme Corp (Pty) Ltd");

    let person_particulars = detail.person_particulars.unwrap();
    assert_eq!(person_particulars.full_name, "Ada Lovelace");
    assert_eq!(
        person_particulars.identity_number.as_deref(),
        Some("80012345678")
    );
    assert_eq!(detail.full_name, "Ada Lovelace");
}

/// A row exactly as a pre-#73 release left it: no use case can produce this
/// today (`finalize_payroll_run` always freezes all three), so it is built
/// with a raw `UPDATE` — the same documented exception
/// `tests/finalized_payroll_read_models.rs` already uses for
/// `snapshot_schema_version`/`salt_version`. Reads back without error, and
/// every one of the three new fields is `None`: visibly, not silently,
/// carrying no frozen particulars.
#[sqlx::test]
async fn a_payroll_finalized_before_this_ticket_reads_back_with_no_frozen_particulars(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;

    let (_run_id, finalized_payroll_id) = finalize_march(&db, &employer_id).await;

    sqlx::query(
        "UPDATE finalized_payroll
         SET snapshot_schema_version = 1,
             employer_particulars_json = NULL,
             person_particulars_json = NULL,
             payslip_template_version = NULL
         WHERE id = $1::uuid",
    )
    .bind(&finalized_payroll_id)
    .execute(&pool)
    .await
    .unwrap();

    let detail = get_finalized_payroll_detail(&db, &employer_id, &finalized_payroll_id)
        .await
        .unwrap();

    assert_eq!(detail.employer_particulars, None);
    assert_eq!(detail.person_particulars, None);
    assert_eq!(detail.payslip_template_version, None);
    // No frozen name to prefer, so the live join is what a legacy row falls
    // back to — the same value it read before issue #73 shipped.
    assert_eq!(detail.full_name, "Ada Lovelace");
}

/// The acceptance criterion names `UPDATE` *and* `DELETE`: adding columns to
/// `finalized_payroll` must not have weakened either half of §6.2's revoke.
/// Migration 0034 restates the whole permission matrix (as every migration
/// since 0017 does), and this is what proves the restatement did not quietly
/// grant back the row-level deletion the table has never allowed.
#[sqlx::test]
async fn the_restricted_role_cannot_delete_a_row_carrying_the_new_frozen_columns(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    let (_run_id, finalized_payroll_id) = finalize_march(&db, &employer_id).await;

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("SET ROLE payroll_app")
        .execute(&mut *conn)
        .await
        .unwrap();

    let result = sqlx::query("DELETE FROM finalized_payroll WHERE id = $1::uuid")
        .bind(&finalized_payroll_id)
        .execute(&mut *conn)
        .await;

    let err = result.expect_err("the restricted role's DELETE must be refused");
    assert!(matches!(
        &err,
        sqlx::Error::Database(db_err) if db_err.code().as_deref() == Some("42501")
    ));

    // Still there, and still carrying what it froze.
    let detail = get_finalized_payroll_detail(&db, &employer_id, &finalized_payroll_id)
        .await
        .unwrap();
    assert_eq!(
        detail.payslip_template_version.as_deref(),
        Some(payroll_app::PAYSLIP_TEMPLATE_VERSION)
    );
}

/// The other two frozen columns, for the same reason: a `payslip_template_version`
/// `UPDATE` proves the table-level revoke, but only naming each column proves
/// no column-scoped grant (0033 restored one on `person.full_name`) leaked
/// onto this table.
#[sqlx::test]
async fn the_restricted_role_cannot_update_the_new_frozen_particulars_columns(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    let (_run_id, finalized_payroll_id) = finalize_march(&db, &employer_id).await;

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("SET ROLE payroll_app")
        .execute(&mut *conn)
        .await
        .unwrap();

    for column in ["employer_particulars_json", "person_particulars_json"] {
        let result = sqlx::query(&format!(
            "UPDATE finalized_payroll SET {column} = '{{}}'::jsonb WHERE id = $1::uuid"
        ))
        .bind(&finalized_payroll_id)
        .execute(&mut *conn)
        .await;

        let err = result.expect_err("the restricted role's UPDATE must be refused");
        assert!(
            matches!(&err, sqlx::Error::Database(db_err) if db_err.code().as_deref() == Some("42501")),
            "{column} was not refused"
        );
    }
}

#[sqlx::test]
async fn the_restricted_role_cannot_update_the_new_frozen_columns(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    let (_run_id, finalized_payroll_id) = finalize_march(&db, &employer_id).await;

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("SET ROLE payroll_app")
        .execute(&mut *conn)
        .await
        .unwrap();

    let result = sqlx::query(
        "UPDATE finalized_payroll SET payslip_template_version = 'tampered' WHERE id = $1::uuid",
    )
    .bind(&finalized_payroll_id)
    .execute(&mut *conn)
    .await;

    let err = result.expect_err("the restricted role's UPDATE must be refused");
    assert!(matches!(
        &err,
        sqlx::Error::Database(db_err) if db_err.code().as_deref() == Some("42501")
    ));
}
