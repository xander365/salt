//! Proves the two Employer-scoped read models issue #57 introduces —
//! `get_finalized_payroll_detail` and `get_finalized_payroll_traces`
//! (§0.29, §0.30) — an Operator opening a payroll that has already been
//! finalized and reading it back.
//!
//! Seam A of parent #49's Testing Decisions: every precondition is built
//! through the same public use cases these functions are read back through.
//! The two raw writes below are the documented exception — they construct a
//! `finalized_payroll` row as an *older release* would have left it, which
//! no public use case can produce, because `finalize_payroll_run` always
//! writes this build's own `SALT_VERSION` and `SNAPSHOT_SCHEMA_VERSION`.
//! The raw read asserts what a read model must *not* have changed, which no
//! public function reports.

use chrono::NaiveDate;
use payroll::{
    EmployerId, EmploymentId, Money, PayPeriod, PeriodEndDay, PriorEmployment, TaxYear,
    UnsupportedDeductionStatus,
};
use payroll_app::{
    EmploymentPerson, PayrollAppError, PayrollRunId, SALT_VERSION, SaltDatabase,
    calculate_payroll_run, create_employer, create_employment, create_ordinary_payroll_run,
    declare_prior_employment, declare_unsupported_deduction_status, finalize_payroll_run,
    get_finalized_payroll_detail, get_finalized_payroll_traces, get_payroll_run_detail,
    record_compensation_terms,
};
use sqlx::PgPool;

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn monthly_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::LastDayOfMonth)
}

/// March 2026 — the calendar month `monthly_schedule()` generates. Every
/// Employment below starts on this period's own first day, so this is its
/// first payable period and no `OpeningBalance` is required (§7.1 branch 1).
fn period() -> PayPeriod {
    PayPeriod::new(date(2026, 3, 1), date(2026, 3, 31)).unwrap()
}

fn pay_date() -> NaiveDate {
    date(2026, 4, 5)
}

const BASIC_PAY_CENTS: i64 = 1_500_000;

async fn an_employer(db: &SaltDatabase, name: &str) -> EmployerId {
    create_employer(db, name, monthly_schedule(), "actor")
        .await
        .unwrap()
}

/// An Employment with every fact `calculate` needs already on record, so
/// the run below reaches `Calculated` with no refusal.
async fn a_fully_declared_employment(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    person: &str,
) -> EmploymentId {
    let (_, employment_id) = create_employment(
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
        Money::from_cents(BASIC_PAY_CENTS).unwrap(),
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
    employment_id
}

/// Creates a March Ordinary run, calculates it, and finalizes it — the
/// whole precondition every test here needs, built through public use cases
/// only. Returns the run and the one member's `FinalizedPayrollId` as the
/// string a route would hand the read model.
async fn a_finalized_payroll(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    person: &str,
) -> (PayrollRunId, EmploymentId, String) {
    let employment_id = a_fully_declared_employment(db, employer_id, person).await;
    let run_id = create_ordinary_payroll_run(db, employer_id, period(), pay_date(), "actor")
        .await
        .unwrap();
    let refusals = calculate_payroll_run(db, &run_id, "calculator")
        .await
        .unwrap();
    assert_eq!(refusals, Vec::new(), "the run must reach Calculated");
    let outcome = finalize_payroll_run(db, &run_id, "finalizer")
        .await
        .unwrap();
    assert_eq!(outcome.finalized.len(), 1);
    let finalized_payroll_id = outcome.finalized[0].1.as_str().to_string();
    (run_id, employment_id, finalized_payroll_id)
}

/// Read raw: `finalized_payroll` is never updated by any use case, so no
/// public function reports whether a read left it alone.
async fn finalized_payroll_row_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM finalized_payroll")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn action_log_entry_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM action_log_entry")
        .fetch_one(pool)
        .await
        .unwrap()
}

// ---- GetFinalizedPayrollDetail ----

#[sqlx::test]
async fn a_finalized_payroll_reads_back_its_figures_period_pay_date_and_salt_version(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db, "Employer").await;
    let (_run_id, employment_id, finalized_payroll_id) =
        a_finalized_payroll(&db, &employer_id, "Ada Lovelace").await;

    let detail = get_finalized_payroll_detail(&db, &employer_id, &finalized_payroll_id)
        .await
        .unwrap();

    assert_eq!(detail.id.as_str(), finalized_payroll_id);
    assert_eq!(detail.employment_id, employment_id);
    assert_eq!(detail.period, period());
    assert_eq!(detail.pay_date, pay_date());
    assert_eq!(detail.salt_version, SALT_VERSION);
    assert_eq!(
        detail.figures.basic_pay,
        Money::from_cents(BASIC_PAY_CENTS).unwrap()
    );
    assert_eq!(detail.figures.taxable_allowances, Money::ZERO);
    assert_eq!(
        detail.figures.gross,
        Money::from_cents(BASIC_PAY_CENTS).unwrap()
    );
    // INV-007: employer SSC is an employer cost and never a deduction from
    // the employee, so it is absent from `total_deductions` and from the
    // gross-minus-deductions identity `net_pay` satisfies.
    assert_eq!(
        detail.figures.total_deductions,
        detail
            .figures
            .paye
            .checked_add(detail.figures.employee_social_security)
            .unwrap()
    );
    assert_eq!(
        detail
            .figures
            .gross
            .checked_sub(detail.figures.total_deductions)
            .unwrap(),
        detail.figures.net_pay
    );
}

/// The figure a screen shows while the run is `Calculated` and the figure it
/// shows once the run is `Finalized` are the same nine, read through one
/// `PayrollFigures::from_calculation` — the point of sharing that shape
/// (issue #57). A finalized read that quietly disagreed with the working
/// read would make a past month unexplainable.
#[sqlx::test]
async fn the_finalized_figures_are_the_ones_the_run_showed_before_it_was_finalized(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db, "Employer").await;
    let employment_id = a_fully_declared_employment(&db, &employer_id, "Ada Lovelace").await;
    let run_id = create_ordinary_payroll_run(&db, &employer_id, period(), pay_date(), "actor")
        .await
        .unwrap();
    calculate_payroll_run(&db, &run_id, "calculator")
        .await
        .unwrap();

    let calculated = get_payroll_run_detail(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    let working_figures = calculated
        .members
        .iter()
        .find(|member| member.employment_id == employment_id)
        .and_then(|member| member.figures)
        .expect("a calculated member carries its figures");

    let outcome = finalize_payroll_run(&db, &run_id, "finalizer")
        .await
        .unwrap();
    let finalized_payroll_id = outcome.finalized[0].1.as_str().to_string();

    let detail = get_finalized_payroll_detail(&db, &employer_id, &finalized_payroll_id)
        .await
        .unwrap();

    assert_eq!(detail.figures, working_figures);
}

/// The `SaltVersion` comes from the finalized row itself and never from the
/// running binary's `SALT_VERSION` (issue #57's own Deep Instructions): the
/// whole point is explaining a figure produced by an older release.
#[sqlx::test]
async fn the_salt_version_is_the_rows_own_and_not_the_running_builds(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db, "Employer").await;
    let (_run_id, _employment_id, finalized_payroll_id) =
        a_finalized_payroll(&db, &employer_id, "Ada Lovelace").await;

    // As an older release would have left the row. No use case can write
    // this, and none may: `finalized_payroll` has no UPDATE grant.
    sqlx::query("UPDATE finalized_payroll SET salt_version = '0.0.1-historic' WHERE id = $1::uuid")
        .bind(&finalized_payroll_id)
        .execute(&pool)
        .await
        .unwrap();

    let detail = get_finalized_payroll_detail(&db, &employer_id, &finalized_payroll_id)
        .await
        .unwrap();

    assert_eq!(detail.salt_version, "0.0.1-historic");
    assert_ne!(detail.salt_version, SALT_VERSION);
}

/// ADR-0012: finalized history is never migrated in place, so a reader
/// branches on the row's own layout version rather than deserializing an
/// unknown layout as if it were the current one.
#[sqlx::test]
async fn a_snapshot_layout_this_build_does_not_know_is_refused_on_both_routes(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db, "Employer").await;
    let (_run_id, _employment_id, finalized_payroll_id) =
        a_finalized_payroll(&db, &employer_id, "Ada Lovelace").await;

    sqlx::query("UPDATE finalized_payroll SET snapshot_schema_version = 2 WHERE id = $1::uuid")
        .bind(&finalized_payroll_id)
        .execute(&pool)
        .await
        .unwrap();

    for result in [
        get_finalized_payroll_detail(&db, &employer_id, &finalized_payroll_id)
            .await
            .err(),
        get_finalized_payroll_traces(&db, &employer_id, &finalized_payroll_id)
            .await
            .err(),
    ] {
        assert!(matches!(
            result,
            Some(PayrollAppError::FinalizedPayrollSnapshotUnreadable {
                schema_version: 2,
                ..
            })
        ));
    }
}

// ---- GetFinalizedPayrollTraces ----

#[sqlx::test]
async fn the_traces_are_the_paye_and_ssc_workings_behind_the_figures(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db, "Employer").await;
    let (_run_id, _employment_id, finalized_payroll_id) =
        a_finalized_payroll(&db, &employer_id, "Ada Lovelace").await;

    let detail = get_finalized_payroll_detail(&db, &employer_id, &finalized_payroll_id)
        .await
        .unwrap();
    let traces = get_finalized_payroll_traces(&db, &employer_id, &finalized_payroll_id)
        .await
        .unwrap();

    let basic_pay = Money::from_cents(BASIC_PAY_CENTS).unwrap();
    assert_eq!(
        traces.paye.this_period_taxable_remuneration,
        detail.figures.taxable_remuneration
    );
    assert_eq!(traces.employee_social_security.basic_pay, basic_pay);
    assert_eq!(traces.employer_social_security.basic_pay, basic_pay);
    // A trace explains its own figure: the base it charged, times its rate,
    // is what the detail reports as that figure.
    assert!(traces.employee_social_security.base <= basic_pay);
    assert!(traces.employer_social_security.base <= basic_pay);
}

// ---- Employer scoping, on both read models (ADR-0017) ----

#[sqlx::test]
async fn another_employers_finalized_payroll_is_not_found_on_both_read_models(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let ours = an_employer(&db, "Ours").await;
    let theirs = an_employer(&db, "Theirs").await;
    let (_run_id, _employment_id, their_finalized_payroll_id) =
        a_finalized_payroll(&db, &theirs, "Ada Lovelace").await;

    let detail = get_finalized_payroll_detail(&db, &ours, &their_finalized_payroll_id).await;
    let traces = get_finalized_payroll_traces(&db, &ours, &their_finalized_payroll_id).await;

    // Byte-identical to the unknown-id refusal below: an id must never be an
    // existence oracle over another Employer's history.
    assert_eq!(
        detail.unwrap_err().to_string(),
        format!("no FinalizedPayroll exists with id {their_finalized_payroll_id}")
    );
    assert_eq!(
        traces.unwrap_err().to_string(),
        format!("no FinalizedPayroll exists with id {their_finalized_payroll_id}")
    );
}

#[sqlx::test]
async fn an_unknown_finalized_payroll_id_is_not_found_on_both_read_models(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db, "Employer").await;
    let unknown = "0199a0f0-0000-7000-8000-000000000000";

    assert!(matches!(
        get_finalized_payroll_detail(&db, &employer_id, unknown).await,
        Err(PayrollAppError::FinalizedPayrollNotFound(_))
    ));
    assert!(matches!(
        get_finalized_payroll_traces(&db, &employer_id, unknown).await,
        Err(PayrollAppError::FinalizedPayrollNotFound(_))
    ));
}

/// A path segment that is not even a UUID is the same not-found refusal, not
/// a database error: the `id = $1::uuid` cast would otherwise turn a
/// client's typo into a 500.
#[sqlx::test]
async fn a_malformed_finalized_payroll_id_is_not_found_rather_than_a_database_error(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db, "Employer").await;

    assert!(matches!(
        get_finalized_payroll_detail(&db, &employer_id, "not-a-uuid").await,
        Err(PayrollAppError::FinalizedPayrollNotFound(_))
    ));
    assert!(matches!(
        get_finalized_payroll_traces(&db, &employer_id, "not-a-uuid").await,
        Err(PayrollAppError::FinalizedPayrollNotFound(_))
    ));
}

// ---- A read is a read (ADR-0004, ADR-0011) ----

/// Spec 2 has no route that updates, reverses or corrects a finalized
/// payroll, and neither read model may become one by accident: reading twice
/// writes no row and logs no action.
#[sqlx::test]
async fn reading_a_finalized_payroll_changes_nothing(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db, "Employer").await;
    let (_run_id, _employment_id, finalized_payroll_id) =
        a_finalized_payroll(&db, &employer_id, "Ada Lovelace").await;

    let rows_before = finalized_payroll_row_count(&pool).await;
    let entries_before = action_log_entry_count(&pool).await;

    let first = get_finalized_payroll_detail(&db, &employer_id, &finalized_payroll_id)
        .await
        .unwrap();
    let first_traces = get_finalized_payroll_traces(&db, &employer_id, &finalized_payroll_id)
        .await
        .unwrap();
    let second = get_finalized_payroll_detail(&db, &employer_id, &finalized_payroll_id)
        .await
        .unwrap();
    let second_traces = get_finalized_payroll_traces(&db, &employer_id, &finalized_payroll_id)
        .await
        .unwrap();

    assert_eq!(first, second);
    assert_eq!(first_traces, second_traces);
    assert_eq!(finalized_payroll_row_count(&pool).await, rows_before);
    assert_eq!(action_log_entry_count(&pool).await, entries_before);
}
