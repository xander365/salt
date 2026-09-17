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
    EmployerParticularsFields, EmploymentPerson, FinalizedPayrollId, FinalizedPayrollLiveness,
    PaymentSummary, PayrollAppError, PayrollFigures, PayrollRegister, PayrollRegisterTotals,
    PayrollRunId, PersonParticularsFields, RunKind, SaltDatabase, add_employment_to_correction_run,
    calculate_payroll_run, correct_person_full_name, create_correction_run, create_employer,
    create_employment, create_ordinary_payroll_run, declare_prior_employment,
    declare_unsupported_deduction_status, finalize_payroll_run, get_finalized_payroll_detail,
    get_payment_summary, get_payroll_register, get_run_payslip_data, record_compensation_terms,
    record_employment_end_date, reverse_finalized_payroll, set_employer_particulars,
    set_person_particulars, set_run_pay_lines,
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

/// Every field of `totals` is exactly the checked sum of that field over
/// `figures` — `Money::ZERO` for none at all.
fn assert_totals_sum(totals: &PayrollRegisterTotals, figures: &[PayrollFigures]) {
    let sum =
        |field: fn(&PayrollFigures) -> Money| figures.iter().map(field).fold(Money::ZERO, add);
    assert_eq!(totals.basic_pay, sum(|f| f.basic_pay));
    assert_eq!(totals.taxable_allowances, sum(|f| f.taxable_allowances));
    assert_eq!(totals.overtime, sum(|f| f.overtime));
    assert_eq!(totals.gross, sum(|f| f.gross));
    assert_eq!(totals.taxable_remuneration, sum(|f| f.taxable_remuneration));
    assert_eq!(totals.paye, sum(|f| f.paye));
    assert_eq!(
        totals.employee_social_security,
        sum(|f| f.employee_social_security)
    );
    assert_eq!(
        totals.employer_social_security,
        sum(|f| f.employer_social_security)
    );
    assert_eq!(totals.medical_aid_premium, sum(|f| f.medical_aid_premium));
    assert_eq!(totals.total_deductions, sum(|f| f.total_deductions));
    assert_eq!(totals.net_pay, sum(|f| f.net_pay));
}

/// A register never disagrees with itself: "as finalized" is the sum of
/// every row, "still live" the sum of the Live rows only.
fn assert_register_reconciles(register: &PayrollRegister) {
    let every: Vec<PayrollFigures> = register.rows.iter().map(|row| row.figures).collect();
    let live: Vec<PayrollFigures> = register
        .rows
        .iter()
        .filter(|row| row.liveness == FinalizedPayrollLiveness::Live)
        .map(|row| row.figures)
        .collect();
    assert_totals_sum(&register.total_as_finalized, &every);
    assert_totals_sum(&register.total_still_live, &live);
}

/// A summary never disagrees with itself: its total is the sum of its rows.
fn assert_summary_reconciles(summary: &PaymentSummary) {
    assert_eq!(
        summary.total_net_pay,
        summary
            .rows
            .iter()
            .fold(Money::ZERO, |total, row| add(total, row.net_pay))
    );
}

/// Reads a register that must exist, and proves it reconciles — every
/// successful read in this file goes through here.
async fn register_of(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    run_id: &PayrollRunId,
) -> PayrollRegister {
    let register = get_payroll_register(db, employer_id, run_id.as_str())
        .await
        .unwrap();
    assert_register_reconciles(&register);
    register
}

/// Reads a summary that must exist, and proves it reconciles.
async fn summary_of(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    run_id: &PayrollRunId,
) -> PaymentSummary {
    let summary = get_payment_summary(db, employer_id, run_id.as_str())
        .await
        .unwrap();
    assert_summary_reconciles(&summary);
    summary
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

    let register = register_of(&db, &employer_id, &run_id).await;
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

    assert_totals_sum(
        &register.total_as_finalized,
        &[register.rows[0].figures, register.rows[1].figures],
    );
    assert_totals_sum(
        &register.total_still_live,
        &[register.rows[0].figures, register.rows[1].figures],
    );

    let summary = summary_of(&db, &employer_id, &run_id).await;
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

    let register = register_of(&db, &employer_id, &run_id).await;
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

    assert_totals_sum(
        &register.total_as_finalized,
        &[row_a.figures, row_b.figures],
    );
    assert_eq!(register.total_still_live.net_pay, row_b.figures.net_pay);
    assert_eq!(register.total_still_live.gross, row_b.figures.gross);

    let summary = summary_of(&db, &employer_id, &run_id).await;
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

    let original_register = register_of(&db, &employer_id, &original_run_id).await;
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

    let correction_register = register_of(&db, &employer_id, &correction_run_id).await;
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

    let correction_summary = summary_of(&db, &employer_id, &correction_run_id).await;
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

    let register = register_of(&db, &employer_id, &run_id).await;
    assert_eq!(register.rows.len(), 1);
    assert_eq!(register.rows[0].full_name, "Ada Lovelace");

    let summary = summary_of(&db, &employer_id, &run_id).await;
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

    let register = register_of(&db, &employer_id, &run_id).await;
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

    let register = register_of(&db, &employer_id, &run_id).await;
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

// ---- Step 7b hardening -------------------------------------------------

fn reversed_reason(liveness: &FinalizedPayrollLiveness) -> &str {
    match liveness {
        FinalizedPayrollLiveness::Reversed { reason, .. } => reason,
        FinalizedPayrollLiveness::Live => panic!("expected a Reversed row, found a Live one"),
    }
}

/// Every row of the run reversed: the register still shows them all, its
/// "still live" total is zero, and the summary lists nobody, totals N$0.00
/// and counts every row as excluded.
#[sqlx::test]
async fn a_run_whose_every_row_is_reversed_has_a_zero_live_total_and_an_empty_summary(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    a_fully_declared_employment(&db, &employer_id, "Bob Marker").await;
    let (run_id, finalized) = finalize_march(&db, &employer_id).await;
    for (_, finalized_payroll_id) in &finalized {
        reverse_finalized_payroll(
            &db,
            finalized_payroll_id,
            "the whole run was wrong",
            "actor",
        )
        .await
        .unwrap();
    }

    let register = register_of(&db, &employer_id, &run_id).await;
    assert_eq!(register.rows.len(), 2);
    for row in &register.rows {
        assert_eq!(reversed_reason(&row.liveness), "the whole run was wrong");
    }
    assert_totals_sum(&register.total_still_live, &[]);
    assert_eq!(register.total_still_live.net_pay, Money::ZERO);
    assert_ne!(register.total_as_finalized.net_pay, Money::ZERO);

    let summary = summary_of(&db, &employer_id, &run_id).await;
    assert!(summary.rows.is_empty());
    assert_eq!(summary.total_net_pay, Money::ZERO);
    assert_eq!(summary.excluded_reversed_count, 2);
}

/// A Replacement that is itself reversed later: the Correction run's
/// register shows it Reversed (still naming what it replaced), and its
/// summary excludes it — a replacement gets no exemption from liveness.
#[sqlx::test]
async fn a_replacement_reversed_later_is_reversed_in_its_register_and_excluded_from_its_summary(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    let (_person_id, employment_id) =
        a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    let (_, finalized) = finalize_march(&db, &employer_id).await;
    let original_id = finalized[0].1.clone();
    reverse_finalized_payroll(&db, &original_id, "March salary was wrong", "actor")
        .await
        .unwrap();
    let (correction_run_id, replacement_id) =
        finalize_a_correction(&db, &employer_id, &employment_id, Some(&original_id)).await;

    reverse_finalized_payroll(
        &db,
        &replacement_id,
        "the correction was wrong too",
        "actor",
    )
    .await
    .unwrap();

    let register = register_of(&db, &employer_id, &correction_run_id).await;
    assert_eq!(register.rows.len(), 1);
    assert_eq!(register.rows[0].finalized_payroll_id, replacement_id);
    assert_eq!(register.rows[0].replaces, Some(original_id));
    assert_eq!(
        reversed_reason(&register.rows[0].liveness),
        "the correction was wrong too"
    );
    assert_eq!(register.total_still_live.net_pay, Money::ZERO);

    let summary = summary_of(&db, &employer_id, &correction_run_id).await;
    assert!(summary.rows.is_empty());
    assert_eq!(summary.excluded_reversed_count, 1);
    assert_eq!(summary.total_net_pay, Money::ZERO);
}

/// A Person's name corrected after finalization changes none of the three
/// outputs: register, summary and batch payslips all print the frozen name.
#[sqlx::test]
async fn a_name_corrected_after_finalization_leaves_all_three_outputs_on_the_frozen_name(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    set_employer_particulars(
        &db,
        &employer_id,
        EmployerParticularsFields {
            registered_name: "Acme Corp (Pty) Ltd".to_string(),
            address_line1: "1 Independence Ave".to_string(),
            address_line2: None,
            city: "Windhoek".to_string(),
            postal_code: Some("10001".to_string()),
            income_tax_number: Some("12345678".to_string()),
            social_security_number: None,
        },
        &[],
        "",
        "operator:alice",
    )
    .await
    .unwrap();
    let (person_id, _) = a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
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

    let register = register_of(&db, &employer_id, &run_id).await;
    assert_eq!(register.rows[0].full_name, "Ada Lovelace");
    let summary = summary_of(&db, &employer_id, &run_id).await;
    assert_eq!(summary.rows[0].full_name, "Ada Lovelace");
    let payslips = get_run_payslip_data(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    assert_eq!(payslips.len(), 1);
    assert_eq!(payslips[0].person_particulars.full_name, "Ada Lovelace");
}

/// A mixed run — a prorated leaver, a member with overtime and medical aid,
/// and a plain member: every register row's figures are exactly the
/// figures `get_finalized_payroll_detail` reads for that row.
#[sqlx::test]
async fn a_mixed_run_register_matches_the_finalized_payroll_detail_row_by_row(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db).await;
    let (_, leaver) = a_fully_declared_employment(&db, &employer_id, "Lena Leaver").await;
    let (_, busy) = a_fully_declared_employment(&db, &employer_id, "Otto Overtime").await;
    a_fully_declared_employment(&db, &employer_id, "Pat Plain").await;
    record_employment_end_date(
        &db,
        &employer_id,
        &leaver,
        date(2026, 3, 15),
        "resigned",
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
        &busy,
        vec![payroll::EarningInstruction::Overtime {
            hours: payroll::OvertimeHours::new(rust_decimal::Decimal::new(12, 0)).unwrap(),
            multiplier: payroll::OvertimeMultiplier::OneAndAHalf,
            label: None,
        }],
        vec![payroll::VoluntaryDeductionInstruction::MedicalAidPremium(
            Money::from_cents(75_000).unwrap(),
        )],
    )
    .await
    .unwrap();
    assert!(
        calculate_payroll_run(&db, &run_id, "calculator")
            .await
            .unwrap()
            .is_empty()
    );
    let finalized = finalize_payroll_run(&db, &run_id, "finalizer")
        .await
        .unwrap()
        .finalized;

    let register = register_of(&db, &employer_id, &run_id).await;
    assert_eq!(register.rows.len(), 3);
    for row in &register.rows {
        let finalized_payroll_id = &finalized
            .iter()
            .find(|(employment_id, _)| employment_id == &row.employment_id)
            .unwrap()
            .1;
        let detail = get_finalized_payroll_detail(&db, &employer_id, finalized_payroll_id.as_str())
            .await
            .unwrap();
        assert_eq!(row.figures, detail.figures);
    }

    let row = |employment_id: &EmploymentId| {
        register
            .rows
            .iter()
            .find(|row| &row.employment_id == employment_id)
            .unwrap()
            .figures
    };
    assert!(row(&leaver).basic_pay < Money::from_cents(1_500_000).unwrap());
    assert_ne!(row(&busy).overtime, Money::ZERO);
    assert_eq!(
        row(&busy).medical_aid_premium,
        Money::from_cents(75_000).unwrap()
    );
}

/// Reversals landing while the outputs are read never yield a register or
/// summary that disagrees with its own rows: each read is one snapshot.
#[sqlx::test]
async fn reads_racing_reversals_always_reconcile(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    for name in ["A", "B", "C", "D", "E", "F"] {
        a_fully_declared_employment(&db, &employer_id, name).await;
    }
    let (run_id, finalized) = finalize_march(&db, &employer_id).await;

    let reverser = {
        let db = SaltDatabase::from_pool(pool.clone());
        tokio::spawn(async move {
            for (_, finalized_payroll_id) in finalized {
                reverse_finalized_payroll(&db, &finalized_payroll_id, "racing", "actor")
                    .await
                    .unwrap();
                tokio::task::yield_now().await;
            }
        })
    };
    let reader = {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = employer_id.clone();
        let run_id = run_id.clone();
        tokio::spawn(async move {
            for _ in 0..40 {
                let register = register_of(&db, &employer_id, &run_id).await;
                assert_eq!(register.rows.len(), 6);
                let summary = summary_of(&db, &employer_id, &run_id).await;
                assert_eq!(summary.rows.len() + summary.excluded_reversed_count, 6);
            }
        })
    };
    reverser.await.unwrap();
    reader.await.unwrap();

    let summary = summary_of(&db, &employer_id, &run_id).await;
    assert_eq!(summary.excluded_reversed_count, 6);
}
