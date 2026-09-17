//! Proves `get_payslip_data` (issue #82, parent #70): everything a Payslip
//! renderer needs for one `FinalizedPayroll`, refused when the frozen
//! particulars issue #73 introduced never froze on this row, and correctly
//! naming reversal and replacement lineage (§6.1, §6.3, CONTEXT.md's own
//! `Reversal`/`Replacement` entries).
//!
//! Fixtures mirror `tests/freeze_particulars_and_template_version.rs` (the
//! particulars fixture) and `tests/correction_run.rs` (the reversal and
//! replacement chain) — both already proven against the public API.

use chrono::NaiveDate;
use payroll::{
    EarningInstruction, EarningLabel, EmployerId, EmploymentId, Money, PayPeriod, PeriodEndDay,
    PriorEmployment, TaxYear, UnsupportedDeductionStatus,
};
use payroll_app::{
    EmployerParticularsFields, EmploymentPerson, FinalizedPayrollId, PAYSLIP_TEMPLATE_VERSION,
    PayLineSource, PayrollAppError, PayrollRunId, PersonParticularsFields, SaltDatabase,
    add_employment_to_correction_run, calculate_payroll_run, create_correction_run,
    create_employer, create_employment, create_ordinary_payroll_run, declare_prior_employment,
    declare_unsupported_deduction_status, finalize_payroll_run, get_payslip_data,
    get_run_payslip_data, record_compensation_terms, reverse_finalized_payroll,
    set_employer_particulars, set_person_particulars, set_run_pay_lines,
};
use sqlx::PgPool;

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn monthly_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::LastDayOfMonth)
}

fn period() -> PayPeriod {
    PayPeriod::new(date(2026, 3, 1), date(2026, 3, 31)).unwrap()
}

async fn an_employer(db: &SaltDatabase) -> EmployerId {
    create_employer(db, "Employer", monthly_schedule(), "actor")
        .await
        .unwrap()
}

fn employer_particulars_fields() -> EmployerParticularsFields {
    EmployerParticularsFields {
        registered_name: "Acme Corp (Pty) Ltd".to_string(),
        address_line1: "1 Independence Ave".to_string(),
        address_line2: None,
        city: "Windhoek".to_string(),
        postal_code: Some("10001".to_string()),
        income_tax_number: Some("12345678".to_string()),
        social_security_number: None,
    }
}

fn person_particulars_fields() -> PersonParticularsFields {
    PersonParticularsFields {
        identity_number: "80012345678".to_string(),
        address_line1: "2 Fidel Castro St".to_string(),
        address_line2: None,
        city: "Swakopmund".to_string(),
        postal_code: Some("9000".to_string()),
    }
}

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

async fn finalize_march(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    employment_id: &EmploymentId,
) -> FinalizedPayrollId {
    let run_id = create_ordinary_payroll_run(db, employer_id, period(), date(2026, 4, 5), "actor")
        .await
        .unwrap();
    calculate_payroll_run(db, &run_id, "calculator")
        .await
        .unwrap();
    let outcome = finalize_payroll_run(db, &run_id, "finalizer")
        .await
        .unwrap();
    outcome
        .finalized
        .into_iter()
        .find(|(id, _)| id == employment_id)
        .expect("the Employment must have finalized")
        .1
}

/// As [`finalize_march`], but for every currently-active Employment at
/// once — the shortest path to a batch worth ordering, and returns the
/// full outcome so a caller can look a given Employment's own id up rather
/// than trust array order.
async fn finalize_march_run(
    db: &SaltDatabase,
    employer_id: &EmployerId,
) -> (PayrollRunId, Vec<(EmploymentId, FinalizedPayrollId)>) {
    let run_id = create_ordinary_payroll_run(db, employer_id, period(), date(2026, 4, 5), "actor")
        .await
        .unwrap();
    calculate_payroll_run(db, &run_id, "calculator")
        .await
        .unwrap();
    let outcome = finalize_payroll_run(db, &run_id, "finalizer")
        .await
        .unwrap();
    (run_id, outcome.finalized)
}

async fn finalize_a_correction(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    employment_id: &EmploymentId,
    replaces: Option<&FinalizedPayrollId>,
) -> FinalizedPayrollId {
    let run_id = create_correction_run(
        db,
        employer_id,
        period(),
        date(2026, 7, 5),
        "the figure was wrong",
        "actor",
    )
    .await
    .unwrap();
    add_employment_to_correction_run(db, &run_id, employment_id, replaces, "actor")
        .await
        .unwrap();
    calculate_payroll_run(db, &run_id, "calculator")
        .await
        .unwrap();
    let outcome = finalize_payroll_run(db, &run_id, "finalizer")
        .await
        .unwrap();
    outcome
        .finalized
        .into_iter()
        .find(|(id, _)| id == employment_id)
        .expect("the Correction's one member must have finalized")
        .1
}

/// The everyday case: every frozen field a Payslip needs is present, and
/// there is no reversal or replacement to report.
#[sqlx::test]
async fn a_fully_frozen_payroll_reads_back_everything_a_payslip_needs(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    let (person_id, employment_id) =
        a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    set_employer_particulars(
        &db,
        &employer_id,
        employer_particulars_fields(),
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
        person_particulars_fields(),
        &[],
        "",
        "operator:alice",
    )
    .await
    .unwrap();

    let finalized_payroll_id = finalize_march(&db, &employer_id, &employment_id).await;

    let data = get_payslip_data(&db, &employer_id, finalized_payroll_id.as_str())
        .await
        .unwrap();

    assert_eq!(data.id, finalized_payroll_id);
    assert_eq!(data.employment_id, employment_id);
    assert_eq!(data.period, period());
    assert_eq!(data.pay_date, date(2026, 4, 5));
    assert_eq!(data.payslip_template_version, PAYSLIP_TEMPLATE_VERSION);
    assert_eq!(
        data.employer_particulars.registered_name,
        "Acme Corp (Pty) Ltd"
    );
    assert_eq!(data.person_particulars.full_name, "Ada Lovelace");
    assert_eq!(
        data.person_particulars.identity_number.as_deref(),
        Some("80012345678")
    );
    assert!(!data.earning_lines.is_empty());
    assert_eq!(data.figures.basic_pay, Money::from_cents(1500000).unwrap());
    assert_eq!(data.replaces, None);
    assert_eq!(data.reversal, None);
}

/// The refusal Deep Instructions demand: a row predating issue #73 names
/// exactly which of the three frozen columns is absent, never merely that
/// something is. Built the same way
/// `tests/freeze_particulars_and_template_version.rs` builds its own
/// "as an older release would have left it" fixture: no public use case can
/// produce this row, so a raw `UPDATE` stands in for one.
#[sqlx::test]
async fn a_payroll_predating_the_freeze_is_refused_naming_what_is_missing(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let (person_id, employment_id) =
        a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    // Both master records exist and are complete today. The refusal below
    // must still happen: a payslip never falls back to a current record
    // when the frozen one is absent.
    set_employer_particulars(
        &db,
        &employer_id,
        employer_particulars_fields(),
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
        person_particulars_fields(),
        &[],
        "",
        "operator:alice",
    )
    .await
    .unwrap();

    let finalized_payroll_id = finalize_march(&db, &employer_id, &employment_id).await;

    sqlx::query(
        "UPDATE finalized_payroll
         SET snapshot_schema_version = 1,
             employer_particulars_json = NULL,
             person_particulars_json = NULL,
             payslip_template_version = NULL
         WHERE id = $1::uuid",
    )
    .bind(finalized_payroll_id.as_str())
    .execute(&pool)
    .await
    .unwrap();

    let err = get_payslip_data(&db, &employer_id, finalized_payroll_id.as_str())
        .await
        .unwrap_err();

    assert_eq!(
        err,
        PayrollAppError::PayslipParticularsNotFrozen {
            finalized_payroll_id: finalized_payroll_id.clone(),
            missing: vec![
                "EmployerParticulars",
                "PersonParticulars",
                "PayslipTemplateVersion",
            ],
        }
    );
}

/// Only one of the three absent: `missing` names only that one, never the
/// other two which are in fact present.
#[sqlx::test]
async fn only_the_columns_actually_absent_are_named(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let (person_id, employment_id) =
        a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    set_person_particulars(
        &db,
        &employer_id,
        &person_id,
        person_particulars_fields(),
        &[],
        "",
        "operator:alice",
    )
    .await
    .unwrap();

    let finalized_payroll_id = finalize_march(&db, &employer_id, &employment_id).await;

    // `EmployerParticulars` was never recorded, so `finalize_payroll_run`
    // itself already froze `None` for it — no raw `UPDATE` needed here.
    let err = get_payslip_data(&db, &employer_id, finalized_payroll_id.as_str())
        .await
        .unwrap_err();

    assert_eq!(
        err,
        PayrollAppError::PayslipParticularsNotFrozen {
            finalized_payroll_id,
            missing: vec!["EmployerParticulars"],
        }
    );
}

/// ADR-0017's own rule, restated for this read model: an id belonging to
/// another Employer reads back exactly as an unknown one would.
#[sqlx::test]
async fn a_finalized_payroll_belonging_to_another_employer_reads_back_as_not_found(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    let (_person_id, employment_id) =
        a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    let finalized_payroll_id = finalize_march(&db, &employer_id, &employment_id).await;

    let other_employer_id = an_employer(&db).await;

    let err = get_payslip_data(&db, &other_employer_id, finalized_payroll_id.as_str())
        .await
        .unwrap_err();

    assert_eq!(
        err,
        PayrollAppError::FinalizedPayrollNotFound(finalized_payroll_id)
    );
}

/// A reversed `FinalizedPayroll` still renders — it is only marked, never
/// refused — and with nothing yet replacing it, names no replacement.
#[sqlx::test]
async fn a_reversed_payroll_with_no_replacement_names_its_reason_and_no_replacement(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    let (person_id, employment_id) =
        a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    set_employer_particulars(
        &db,
        &employer_id,
        employer_particulars_fields(),
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
        person_particulars_fields(),
        &[],
        "",
        "operator:alice",
    )
    .await
    .unwrap();
    let finalized_payroll_id = finalize_march(&db, &employer_id, &employment_id).await;

    reverse_finalized_payroll(
        &db,
        &finalized_payroll_id,
        "March salary was wrong",
        "actor",
    )
    .await
    .unwrap();

    let data = get_payslip_data(&db, &employer_id, finalized_payroll_id.as_str())
        .await
        .unwrap();

    let reversal = data.reversal.expect("a reversed row must carry a reversal");
    assert_eq!(reversal.reason, "March salary was wrong");
    assert_eq!(reversal.replacement_id, None);
    assert_eq!(data.replaces, None);
    // `reversed_at` defaults to `now()` (migration 0011) — not exact, but
    // must be recent, the same tolerance `reverse_finalized_payroll.rs`'s
    // own test already applies to the same column.
    let elapsed = chrono::Utc::now() - reversal.reversed_at;
    assert!(
        elapsed >= chrono::Duration::zero() && elapsed < chrono::Duration::minutes(1),
        "{:?}",
        reversal.reversed_at
    );
}

/// The full chain: the reversed record names the Correction that replaced
/// it, and the Correction names what it replaced — both directions of
/// CONTEXT.md's own `Replacement` entry.
#[sqlx::test]
async fn a_replaced_payroll_and_its_replacement_name_each_other(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    let (person_id, employment_id) =
        a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    set_employer_particulars(
        &db,
        &employer_id,
        employer_particulars_fields(),
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
        person_particulars_fields(),
        &[],
        "",
        "operator:alice",
    )
    .await
    .unwrap();
    let original_id = finalize_march(&db, &employer_id, &employment_id).await;
    reverse_finalized_payroll(&db, &original_id, "March salary was wrong", "actor")
        .await
        .unwrap();

    let replacement_id =
        finalize_a_correction(&db, &employer_id, &employment_id, Some(&original_id)).await;

    let original = get_payslip_data(&db, &employer_id, original_id.as_str())
        .await
        .unwrap();
    let reversal = original
        .reversal
        .expect("the original must still carry its reversal");
    assert_eq!(reversal.replacement_id, Some(replacement_id.clone()));

    let replacement = get_payslip_data(&db, &employer_id, replacement_id.as_str())
        .await
        .unwrap();
    assert_eq!(replacement.replaces, Some(original_id));
    assert_eq!(replacement.reversal, None);
}

/// The frozen pay-line provenance (issue #80, issue #82 review): a one-off
/// allowance typed onto this run freezes as a `FrozenPayLine` with source
/// `OneOff`, read back on [`PayslipData::pay_line_provenance`] — never
/// rebuilt from a current `StandingPayItem`, since there is none behind a
/// one-off line to rebuild it from.
#[sqlx::test]
async fn a_one_off_allowances_frozen_source_comes_through_on_the_payslip(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    let (person_id, employment_id) =
        a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    set_employer_particulars(
        &db,
        &employer_id,
        employer_particulars_fields(),
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
        person_particulars_fields(),
        &[],
        "",
        "operator:alice",
    )
    .await
    .unwrap();

    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, period(), date(2026, 4, 5), "actor")
            .await
            .unwrap();
    set_run_pay_lines(
        &db,
        &run_id,
        &employment_id,
        vec![EarningInstruction::TaxableAllowance {
            amount: Money::from_cents(50_000).unwrap(),
            label: Some(EarningLabel::new("Standby allowance").unwrap()),
        }],
        vec![],
    )
    .await
    .unwrap();
    calculate_payroll_run(&db, &run_id, "calculator")
        .await
        .unwrap();
    let outcome = finalize_payroll_run(&db, &run_id, "finalizer")
        .await
        .unwrap();
    let finalized_payroll_id = outcome
        .finalized
        .into_iter()
        .find(|(id, _)| id == &employment_id)
        .expect("the Employment must have finalized")
        .1;

    let data = get_payslip_data(&db, &employer_id, finalized_payroll_id.as_str())
        .await
        .unwrap();

    let provenance = data
        .pay_line_provenance
        .expect("issue #80 shipped before this row ever finalized");
    let allowance = provenance
        .iter()
        .find(|line| line.pay_line.as_earning().is_some())
        .expect("the one-off allowance froze its own provenance entry");
    assert_eq!(allowance.source, PayLineSource::OneOff);
    assert_eq!(allowance.standing_pay_item_id, None);
    assert_eq!(allowance.override_reason, None);
    assert_eq!(allowance.removed_reason, None);
}

/// `get_run_payslip_data` (issue #83): every member of a finalized run, in
/// `ORDER BY employment_id` — Ada was created first, so she prints first.
#[sqlx::test]
async fn get_run_payslip_data_reads_every_row_in_member_order(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    set_employer_particulars(
        &db,
        &employer_id,
        employer_particulars_fields(),
        &[],
        "",
        "operator:alice",
    )
    .await
    .unwrap();
    let (ada_person, ada_employment) =
        a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    set_person_particulars(
        &db,
        &employer_id,
        &ada_person,
        person_particulars_fields(),
        &[],
        "",
        "operator:alice",
    )
    .await
    .unwrap();
    let (bob_person, bob_employment) =
        a_fully_declared_employment(&db, &employer_id, "Bob Marley").await;
    set_person_particulars(
        &db,
        &employer_id,
        &bob_person,
        person_particulars_fields(),
        &[],
        "",
        "operator:alice",
    )
    .await
    .unwrap();

    let (run_id, finalized) = finalize_march_run(&db, &employer_id).await;
    let ada_finalized_id = finalized
        .iter()
        .find(|(id, _)| *id == ada_employment)
        .unwrap()
        .1
        .clone();
    let bob_finalized_id = finalized
        .iter()
        .find(|(id, _)| *id == bob_employment)
        .unwrap()
        .1
        .clone();

    let rows = get_run_payslip_data(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();

    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].employment_id, ada_employment);
    assert_eq!(rows[0].id, ada_finalized_id);
    assert_eq!(rows[1].employment_id, bob_employment);
    assert_eq!(rows[1].id, bob_finalized_id);
}

/// A Correction's replacement row belongs to its own run, never the
/// original — `get_run_payslip_data` on the original run must keep showing
/// only its own two members, the reversed original included, and never the
/// replacement a later run produced.
#[sqlx::test]
async fn get_run_payslip_data_scopes_to_its_own_run_even_after_a_correction(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    set_employer_particulars(
        &db,
        &employer_id,
        employer_particulars_fields(),
        &[],
        "",
        "operator:alice",
    )
    .await
    .unwrap();
    let (ada_person, ada_employment) =
        a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    set_person_particulars(
        &db,
        &employer_id,
        &ada_person,
        person_particulars_fields(),
        &[],
        "",
        "operator:alice",
    )
    .await
    .unwrap();
    let (bob_person, _bob_employment) =
        a_fully_declared_employment(&db, &employer_id, "Bob Marley").await;
    set_person_particulars(
        &db,
        &employer_id,
        &bob_person,
        person_particulars_fields(),
        &[],
        "",
        "operator:alice",
    )
    .await
    .unwrap();

    let (run_id, finalized) = finalize_march_run(&db, &employer_id).await;
    let ada_finalized_id = finalized
        .iter()
        .find(|(id, _)| *id == ada_employment)
        .unwrap()
        .1
        .clone();

    reverse_finalized_payroll(&db, &ada_finalized_id, "March salary was wrong", "actor")
        .await
        .unwrap();
    let replacement_id =
        finalize_a_correction(&db, &employer_id, &ada_employment, Some(&ada_finalized_id)).await;

    let rows = get_run_payslip_data(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    let ids: Vec<FinalizedPayrollId> = rows.iter().map(|row| row.id.clone()).collect();

    assert_eq!(
        rows.len(),
        2,
        "the correction's own row belongs to a different run"
    );
    assert!(ids.contains(&ada_finalized_id));
    assert!(!ids.contains(&replacement_id));
}

/// **Missing particulars in a batch refuse the whole batch**, naming the
/// *first* row in member order that lacks them — Ada's row here, never
/// Bob's, since Ada was created first. No public use case leaves a
/// fully-declared row missing a frozen particular (the same reasoning
/// `a_payroll_predating_the_freeze_is_refused_naming_what_is_missing` above
/// already applies to one row): a raw `UPDATE` stands in for the
/// "predates issue #73" state on Ada's row only, so this proves the
/// refusal names the first offending row, not merely *a* row.
#[sqlx::test]
async fn a_batchs_first_missing_row_refuses_the_whole_batch(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    set_employer_particulars(
        &db,
        &employer_id,
        employer_particulars_fields(),
        &[],
        "",
        "operator:alice",
    )
    .await
    .unwrap();
    let (ada_person, ada_employment) =
        a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    set_person_particulars(
        &db,
        &employer_id,
        &ada_person,
        person_particulars_fields(),
        &[],
        "",
        "operator:alice",
    )
    .await
    .unwrap();
    let (bob_person, _bob_employment) =
        a_fully_declared_employment(&db, &employer_id, "Bob Marley").await;
    set_person_particulars(
        &db,
        &employer_id,
        &bob_person,
        person_particulars_fields(),
        &[],
        "",
        "operator:alice",
    )
    .await
    .unwrap();

    let (run_id, finalized) = finalize_march_run(&db, &employer_id).await;
    let ada_finalized_id = finalized
        .iter()
        .find(|(id, _)| *id == ada_employment)
        .unwrap()
        .1
        .clone();

    sqlx::query("UPDATE finalized_payroll SET payslip_template_version = NULL WHERE id = $1::uuid")
        .bind(ada_finalized_id.as_str())
        .execute(&pool)
        .await
        .unwrap();

    let err = get_run_payslip_data(&db, &employer_id, run_id.as_str())
        .await
        .unwrap_err();

    assert_eq!(
        err,
        PayrollAppError::PayslipParticularsNotFrozen {
            finalized_payroll_id: ada_finalized_id,
            missing: vec!["PayslipTemplateVersion"],
        }
    );
}

/// A Draft (never calculated) run has no `FinalizedPayroll` rows for a
/// batch to read.
#[sqlx::test]
async fn get_run_payslip_data_refuses_a_run_that_has_not_finalized(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, period(), date(2026, 4, 5), "actor")
            .await
            .unwrap();

    let err = get_run_payslip_data(&db, &employer_id, run_id.as_str())
        .await
        .unwrap_err();

    assert_eq!(err, PayrollAppError::PayrollRunNotFinalized(run_id));
}

/// ADR-0017's own rule, restated for this read model: a run id belonging to
/// another Employer reads back exactly as an unknown one would.
#[sqlx::test]
async fn get_run_payslip_data_refuses_a_run_belonging_to_another_employer(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    let (_person_id, _employment_id) =
        a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    let (run_id, _finalized) = finalize_march_run(&db, &employer_id).await;

    let other_employer_id = an_employer(&db).await;

    let err = get_run_payslip_data(&db, &other_employer_id, run_id.as_str())
        .await
        .unwrap_err();

    assert_eq!(err, PayrollAppError::PayrollRunNotFound(run_id));
}
