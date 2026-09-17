//! Proves `get_payroll_register` and `get_payment_summary` (issue #83,
//! parent #70 §D-9, §D-10): every row a finalized PayrollRun produced, its
//! liveness and replacement lineage, and the two read models' totals — all
//! read from the frozen `payroll_calculation_json` snapshot, never
//! recomputed.
//!
//! Fixtures mirror `tests/payslip_data.rs` (the particulars fixture, the
//! reversal and replacement chain) — both already proven against the public
//! API.

use chrono::NaiveDate;
use payroll::{
    EmployerId, EmploymentId, Money, PayPeriod, PeriodEndDay, PriorEmployment, TaxYear,
    UnsupportedDeductionStatus,
};
use payroll_app::{
    EmploymentPerson, FinalizedPayrollId, FinalizedPayrollLiveness, PayrollAppError,
    PayrollFigures, PayrollRegisterTotals, PayrollRunId, PersonParticularsFields, RunKind,
    SaltDatabase, add_employment_to_correction_run, calculate_payroll_run,
    correct_person_full_name, create_correction_run, create_employer, create_employment,
    create_ordinary_payroll_run, declare_prior_employment, declare_unsupported_deduction_status,
    finalize_payroll_run, get_finalized_payroll_detail, get_payment_summary, get_payroll_register,
    record_compensation_terms, reverse_finalized_payroll, set_person_particulars,
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

/// Finalizes every active member of `employer_id`'s March run at once,
/// paired with the `FinalizedPayrollId` each Employment produced (issue
/// #50's own `finalized` list) — used by every test with more than one
/// member.
async fn finalize_march(
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
) -> (PayrollRunId, FinalizedPayrollId) {
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
    let finalized_payroll_id = outcome
        .finalized
        .into_iter()
        .find(|(id, _)| id == employment_id)
        .expect("the Correction's one member must have finalized")
        .1;
    (run_id, finalized_payroll_id)
}

fn add(a: Money, b: Money) -> Money {
    a.checked_add(b)
        .expect("test fixture figures never overflow Money")
}

/// The eleven fields of a [`PayrollRegisterTotals`] are each exactly the sum
/// of the same field across `a` and `b` — the whole point of a totals-summing
/// helper next to the type, checked field by field rather than trusted.
fn assert_totals_are_the_sum_of(
    totals: &PayrollRegisterTotals,
    a: &PayrollFigures,
    b: &PayrollFigures,
) {
    assert_eq!(totals.basic_pay, add(a.basic_pay, b.basic_pay));
    assert_eq!(
        totals.taxable_allowances,
        add(a.taxable_allowances, b.taxable_allowances)
    );
    assert_eq!(totals.overtime, add(a.overtime, b.overtime));
    assert_eq!(totals.gross, add(a.gross, b.gross));
    assert_eq!(
        totals.taxable_remuneration,
        add(a.taxable_remuneration, b.taxable_remuneration)
    );
    assert_eq!(totals.paye, add(a.paye, b.paye));
    assert_eq!(
        totals.employee_social_security,
        add(a.employee_social_security, b.employee_social_security)
    );
    assert_eq!(
        totals.employer_social_security,
        add(a.employer_social_security, b.employer_social_security)
    );
    assert_eq!(
        totals.medical_aid_premium,
        add(a.medical_aid_premium, b.medical_aid_premium)
    );
    assert_eq!(
        totals.total_deductions,
        add(a.total_deductions, b.total_deductions)
    );
    assert_eq!(totals.net_pay, add(a.net_pay, b.net_pay));
}

/// Two Live members: both read models show both rows, in employment-id
/// order, and the register's own totals equal the plain sum of the rows.
#[sqlx::test]
async fn two_finalized_members_appear_in_the_register_and_summary(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    let (_person_a, employment_a) =
        a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    let (_person_b, employment_b) =
        a_fully_declared_employment(&db, &employer_id, "Bob Marker").await;

    let (run_id, _) = finalize_march(&db, &employer_id).await;

    let register = get_payroll_register(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    assert_eq!(register.payroll_run_id, run_id);
    assert_eq!(register.kind, RunKind::Ordinary);
    assert_eq!(register.period, period());
    assert_eq!(register.pay_date, date(2026, 4, 5));
    assert_eq!(register.rows.len(), 2);
    // `employment.id` is a UUIDv7, lexically sortable by creation time
    // (ids.rs), so Ada (created first) sorts before Bob.
    assert_eq!(register.rows[0].employment_id, employment_a);
    assert_eq!(register.rows[1].employment_id, employment_b);
    assert_eq!(register.rows[0].liveness, FinalizedPayrollLiveness::Live);
    assert_eq!(register.rows[1].liveness, FinalizedPayrollLiveness::Live);
    assert_eq!(register.rows[0].replaces, None);
    assert_eq!(register.rows[1].replaces, None);

    assert_totals_are_the_sum_of(
        &register.total_as_finalized,
        &register.rows[0].figures,
        &register.rows[1].figures,
    );
    assert_totals_are_the_sum_of(
        &register.total_still_live,
        &register.rows[0].figures,
        &register.rows[1].figures,
    );

    let summary = get_payment_summary(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    assert_eq!(summary.rows.len(), 2);
    assert_eq!(summary.excluded_reversed_count, 0);
    assert_eq!(
        summary.total_net_pay,
        add(
            register.rows[0].figures.net_pay,
            register.rows[1].figures.net_pay
        )
    );
}

/// Reversing one of two Live rows leaves it in the register, marked
/// Reversed with its reason, `total_as_finalized` unchanged and
/// `total_still_live` down by exactly that row; the summary drops it and
/// counts it as excluded.
#[sqlx::test]
async fn reversing_a_row_keeps_it_in_the_register_but_drops_it_from_the_summary(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    let (_person_a, employment_a) =
        a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    let (_person_b, employment_b) =
        a_fully_declared_employment(&db, &employer_id, "Bob Marker").await;

    let (run_id, finalized) = finalize_march(&db, &employer_id).await;
    let finalized_payroll_id_a = finalized
        .iter()
        .find(|(id, _)| id == &employment_a)
        .unwrap()
        .1
        .clone();

    reverse_finalized_payroll(
        &db,
        &finalized_payroll_id_a,
        "March salary was wrong",
        "actor",
    )
    .await
    .unwrap();

    let register = get_payroll_register(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    let row_a = register
        .rows
        .iter()
        .find(|row| row.employment_id == employment_a)
        .unwrap();
    let row_b = register
        .rows
        .iter()
        .find(|row| row.employment_id == employment_b)
        .unwrap();
    assert_eq!(
        row_a.liveness,
        FinalizedPayrollLiveness::Reversed {
            reason: "March salary was wrong".to_string(),
            reversed_at: match &row_a.liveness {
                FinalizedPayrollLiveness::Reversed { reversed_at, .. } => *reversed_at,
                FinalizedPayrollLiveness::Live => unreachable!(),
            },
            replaced_by: None,
        }
    );
    assert_eq!(row_b.liveness, FinalizedPayrollLiveness::Live);

    assert_totals_are_the_sum_of(&register.total_as_finalized, &row_a.figures, &row_b.figures);
    assert_eq!(register.total_still_live.net_pay, row_b.figures.net_pay);
    assert_eq!(register.total_still_live.gross, row_b.figures.gross);

    let summary = get_payment_summary(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    assert_eq!(summary.rows.len(), 1);
    assert_eq!(summary.rows[0].employment_id, employment_b);
    assert_eq!(summary.excluded_reversed_count, 1);
    assert_eq!(summary.total_net_pay, row_b.figures.net_pay);
}

/// The full chain: the original run's register names the Correction that
/// replaced its reversed row, and the Correction run's own register and
/// summary show exactly the replacement, naming what it replaces, with its
/// full net pay (issue #83 acceptance criterion 4).
#[sqlx::test]
async fn a_replaced_row_names_its_replacement_both_ways(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    let (_person_id, employment_id) =
        a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;

    let (original_run_id, finalized) = finalize_march(&db, &employer_id).await;
    let original_id = finalized[0].1.clone();
    reverse_finalized_payroll(&db, &original_id, "March salary was wrong", "actor")
        .await
        .unwrap();

    let (correction_run_id, replacement_id) =
        finalize_a_correction(&db, &employer_id, &employment_id, Some(&original_id)).await;

    let original_register = get_payroll_register(&db, &employer_id, original_run_id.as_str())
        .await
        .unwrap();
    assert_eq!(original_register.rows.len(), 1);
    assert_eq!(
        original_register.rows[0].liveness,
        FinalizedPayrollLiveness::Reversed {
            reason: "March salary was wrong".to_string(),
            reversed_at: match &original_register.rows[0].liveness {
                FinalizedPayrollLiveness::Reversed { reversed_at, .. } => *reversed_at,
                FinalizedPayrollLiveness::Live => unreachable!(),
            },
            replaced_by: Some(replacement_id.clone()),
        }
    );

    let correction_register = get_payroll_register(&db, &employer_id, correction_run_id.as_str())
        .await
        .unwrap();
    assert_eq!(correction_register.kind, RunKind::Correction);
    assert_eq!(correction_register.rows.len(), 1);
    assert_eq!(
        correction_register.rows[0].finalized_payroll_id,
        replacement_id
    );
    assert_eq!(
        correction_register.rows[0].replaces,
        Some(original_id.clone())
    );
    assert_eq!(
        correction_register.rows[0].liveness,
        FinalizedPayrollLiveness::Live
    );

    let correction_summary = get_payment_summary(&db, &employer_id, correction_run_id.as_str())
        .await
        .unwrap();
    assert_eq!(correction_summary.rows.len(), 1);
    assert_eq!(correction_summary.rows[0].replaces, Some(original_id));
    assert_eq!(
        correction_summary.rows[0].net_pay,
        correction_register.rows[0].figures.net_pay
    );
}

/// A row that never froze `PersonParticulars` (pre-#73) still reads back
/// fully: only the figures and a name are needed, and the name falls back
/// to the live Person.
#[sqlx::test]
async fn a_row_with_no_frozen_particulars_still_reads_back_with_the_live_name(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    let (_person_id, _employment_id) =
        a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    // Deliberately no `set_employer_particulars`/`set_person_particulars`
    // call: this run finalizes with nothing frozen for either.

    let (run_id, _) = finalize_march(&db, &employer_id).await;

    let register = get_payroll_register(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    assert_eq!(register.rows.len(), 1);
    assert_eq!(register.rows[0].full_name, "Ada Lovelace");

    let summary = get_payment_summary(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    assert_eq!(summary.rows[0].full_name, "Ada Lovelace");
}

/// The frozen name wins over a later correction to the live Person — the
/// same rule `get_finalized_payroll_detail` already follows, restated for
/// the register.
#[sqlx::test]
async fn the_frozen_name_wins_over_a_later_correction(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    let (person_id, _employment_id) =
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

    let (run_id, _) = finalize_march(&db, &employer_id).await;

    correct_person_full_name(
        &db,
        &employer_id,
        &person_id,
        "Ada King, Countess of Lovelace",
        &[period()],
        "full legal name recorded",
        "operator:alice",
    )
    .await
    .unwrap();

    let register = get_payroll_register(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    assert_eq!(register.rows[0].full_name, "Ada Lovelace");
}

/// A run that has not yet finalized has no outputs, whether it is still
/// Draft or already Calculated.
#[sqlx::test]
async fn an_unfinalized_run_is_refused_as_not_finalized(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;

    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, period(), date(2026, 4, 5), "actor")
            .await
            .unwrap();

    assert_eq!(
        get_payroll_register(&db, &employer_id, run_id.as_str())
            .await
            .unwrap_err(),
        PayrollAppError::PayrollRunNotFinalized(run_id.clone())
    );
    assert_eq!(
        get_payment_summary(&db, &employer_id, run_id.as_str())
            .await
            .unwrap_err(),
        PayrollAppError::PayrollRunNotFinalized(run_id.clone())
    );

    calculate_payroll_run(&db, &run_id, "calculator")
        .await
        .unwrap();

    assert_eq!(
        get_payroll_register(&db, &employer_id, run_id.as_str())
            .await
            .unwrap_err(),
        PayrollAppError::PayrollRunNotFinalized(run_id.clone())
    );
    assert_eq!(
        get_payment_summary(&db, &employer_id, run_id.as_str())
            .await
            .unwrap_err(),
        PayrollAppError::PayrollRunNotFinalized(run_id)
    );
}

/// ADR-0017's own rule: an id belonging to another Employer, and an id
/// naming nothing at all, both read back exactly the same refusal.
#[sqlx::test]
async fn an_unknown_or_cross_employer_run_id_is_refused_as_not_found(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    let (run_id, _) = finalize_march(&db, &employer_id).await;

    let other_employer_id = an_employer(&db).await;
    assert_eq!(
        get_payroll_register(&db, &other_employer_id, run_id.as_str())
            .await
            .unwrap_err(),
        PayrollAppError::PayrollRunNotFound(run_id.clone())
    );
    assert_eq!(
        get_payment_summary(&db, &other_employer_id, run_id.as_str())
            .await
            .unwrap_err(),
        PayrollAppError::PayrollRunNotFound(run_id)
    );

    let random_id = uuid::Uuid::new_v4().to_string();
    let err = get_payroll_register(&db, &employer_id, &random_id)
        .await
        .unwrap_err();
    assert!(matches!(err, PayrollAppError::PayrollRunNotFound(_)));
    let err = get_payment_summary(&db, &employer_id, &random_id)
        .await
        .unwrap_err();
    assert!(matches!(err, PayrollAppError::PayrollRunNotFound(_)));
}

/// Figures on a register row are read from the exact same frozen snapshot
/// [`get_finalized_payroll_detail`] reads — never recomputed, never a
/// second decode that could drift from it.
#[sqlx::test]
async fn register_figures_match_the_finalized_payroll_detail(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    let (_person_id, employment_id) =
        a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;

    let (run_id, finalized) = finalize_march(&db, &employer_id).await;
    let finalized_payroll_id = finalized[0].1.clone();

    let register = get_payroll_register(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    let row = register
        .rows
        .iter()
        .find(|row| row.employment_id == employment_id)
        .unwrap();

    let detail = get_finalized_payroll_detail(&db, &employer_id, finalized_payroll_id.as_str())
        .await
        .unwrap();

    assert_eq!(row.figures, detail.figures);
}
