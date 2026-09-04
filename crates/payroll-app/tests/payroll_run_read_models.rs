//! Proves the two Employer-scoped read models issue #53 introduces —
//! `list_payroll_runs` and `get_payroll_run_detail` — and the
//! `verify_payroll_run_belongs_to_employer` check the earnings route needs
//! because `set_run_earnings` takes no `EmployerId` of its own.
//!
//! Seam A of parent #49's Testing Decisions: every precondition is built
//! through the same public use cases these functions are read back through,
//! never raw SQL. The two raw reads below assert what a read model must
//! *not* have changed, which no public function reports.

use chrono::NaiveDate;
use payroll::{DayOfMonth, Earning, EmployerId, EmploymentId, Money, PayPeriod, PeriodEndDay};
use payroll_app::{
    EmploymentPerson, PayrollAppError, PayrollRunId, RunStatus, SaltDatabase, create_employer,
    create_employment, create_ordinary_payroll_run, get_payroll_run_detail, list_payroll_runs,
    remove_employment_from_run, set_run_earnings, verify_payroll_run_belongs_to_employer,
};
use sqlx::PgPool;

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// A 26th-to-25th monthly schedule, matching the other tests in this crate.
fn twenty_sixth_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap()))
}

/// 2026-01-26 to 2026-02-25, one of `twenty_sixth_schedule()`'s own periods.
fn february_period() -> PayPeriod {
    PayPeriod::new(date(2026, 1, 26), date(2026, 2, 25)).unwrap()
}

/// 2026-02-26 to 2026-03-25, the period immediately after
/// [`february_period`].
fn march_period() -> PayPeriod {
    PayPeriod::new(date(2026, 2, 26), date(2026, 3, 25)).unwrap()
}

async fn an_employer(db: &SaltDatabase, name: &str) -> EmployerId {
    create_employer(db, name, twenty_sixth_schedule(), "actor")
        .await
        .unwrap()
}

async fn an_employment(db: &SaltDatabase, employer_id: &EmployerId, person: &str) -> EmploymentId {
    create_employment(
        db,
        employer_id,
        EmploymentPerson::New(person.to_string()),
        date(2026, 1, 1),
        None,
        "actor",
    )
    .await
    .unwrap()
    .1
}

async fn a_run(db: &SaltDatabase, employer_id: &EmployerId, period: PayPeriod) -> PayrollRunId {
    create_ordinary_payroll_run(db, employer_id, period, date(2026, 3, 1), "actor")
        .await
        .unwrap()
}

/// The run's stored status, read raw: no public function reports it without
/// also being one of the read models under test here.
async fn stored_status(pool: &PgPool, run_id: &PayrollRunId) -> String {
    sqlx::query_scalar("SELECT status FROM payroll_run WHERE id = $1::uuid")
        .bind(run_id.as_str())
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

// ---- ListPayrollRuns ----

#[sqlx::test]
async fn an_employer_with_no_runs_lists_nothing(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db, "Employer").await;

    assert_eq!(list_payroll_runs(&db, &employer_id).await.unwrap(), vec![]);
}

#[sqlx::test]
async fn a_listed_run_carries_its_period_pay_date_and_status(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db, "Employer").await;
    let run_id = a_run(&db, &employer_id, february_period()).await;

    let runs = list_payroll_runs(&db, &employer_id).await.unwrap();

    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, run_id);
    assert_eq!(runs[0].period, february_period());
    assert_eq!(runs[0].pay_date, date(2026, 3, 1));
    assert_eq!(runs[0].status, RunStatus::Draft);
}

/// ADR-0017's second layer: the filter is in the SQL, so another Employer's
/// run is not merely unmentioned by the caller — it is never read.
#[sqlx::test]
async fn another_employers_run_is_not_listed(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let ours = an_employer(&db, "Ours").await;
    let theirs = an_employer(&db, "Theirs").await;
    let our_run = a_run(&db, &ours, february_period()).await;
    let their_run = a_run(&db, &theirs, february_period()).await;

    let listed: Vec<PayrollRunId> = list_payroll_runs(&db, &ours)
        .await
        .unwrap()
        .into_iter()
        .map(|run| run.id)
        .collect();

    assert_eq!(listed, vec![our_run]);
    assert!(!listed.contains(&their_run));
}

#[sqlx::test]
async fn every_run_the_employer_has_is_listed(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db, "Employer").await;
    let february = a_run(&db, &employer_id, february_period()).await;
    let march = a_run(&db, &employer_id, march_period()).await;

    let listed: Vec<PayrollRunId> = list_payroll_runs(&db, &employer_id)
        .await
        .unwrap()
        .into_iter()
        .map(|run| run.id)
        .collect();

    assert_eq!(listed, vec![february, march]);
}

// ---- GetPayrollRunDetail ----

#[sqlx::test]
async fn a_runs_detail_names_every_member_and_their_earning_lines(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db, "Employer").await;
    let employment_id = an_employment(&db, &employer_id, "Ada Lovelace").await;
    let run_id = a_run(&db, &employer_id, february_period()).await;
    set_run_earnings(
        &db,
        &run_id,
        &employment_id,
        vec![
            Earning::TaxableAllowance(Money::from_cents(5000).unwrap()),
            Earning::TaxableAllowance(Money::from_cents(2500).unwrap()),
        ],
    )
    .await
    .unwrap();

    let detail = get_payroll_run_detail(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();

    assert_eq!(detail.id, run_id);
    assert_eq!(detail.period, february_period());
    assert_eq!(detail.pay_date, date(2026, 3, 1));
    assert_eq!(detail.status, RunStatus::Draft);
    assert_eq!(detail.members.len(), 1);
    assert_eq!(detail.members[0].employment_id, employment_id);
    assert_eq!(detail.members[0].full_name, "Ada Lovelace");
    assert_eq!(
        detail.members[0].earnings,
        vec![
            Earning::TaxableAllowance(Money::from_cents(5000).unwrap()),
            Earning::TaxableAllowance(Money::from_cents(2500).unwrap()),
        ]
    );
}

/// A member with no Earning lines is still a member: the `LEFT JOIN LATERAL`
/// is what keeps someone nobody has typed an allowance for on the screen an
/// Operator checks for omissions.
#[sqlx::test]
async fn a_member_with_no_earning_lines_is_still_a_member(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db, "Employer").await;
    let employment_id = an_employment(&db, &employer_id, "Ada Lovelace").await;
    let run_id = a_run(&db, &employer_id, february_period()).await;

    let detail = get_payroll_run_detail(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();

    assert_eq!(detail.members.len(), 1);
    assert_eq!(detail.members[0].employment_id, employment_id);
    assert_eq!(detail.members[0].earnings, vec![]);
}

/// A run whose whole membership has been removed still has a period, a pay
/// date and a status to show. Reading members separately from the run's own
/// row is what stops an empty run reading as one that does not exist.
#[sqlx::test]
async fn a_run_whose_only_member_was_removed_still_reads_back(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db, "Employer").await;
    let employment_id = an_employment(&db, &employer_id, "Ada Lovelace").await;
    let run_id = a_run(&db, &employer_id, february_period()).await;
    remove_employment_from_run(&db, &run_id, &employment_id, "on unpaid leave", "reviewer")
        .await
        .unwrap();

    let detail = get_payroll_run_detail(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();

    assert_eq!(detail.period, february_period());
    assert_eq!(detail.status, RunStatus::Draft);
    assert_eq!(detail.members, vec![]);
}

/// Reading is reading. A GET never runs the calculator and never moves a run
/// out of `Draft` (parent #49's third `blockers` limit, which this read model
/// is the one that could break).
#[sqlx::test]
async fn reading_a_runs_detail_changes_nothing(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db, "Employer").await;
    an_employment(&db, &employer_id, "Ada Lovelace").await;
    let run_id = a_run(&db, &employer_id, february_period()).await;
    let entries_before = action_log_entry_count(&pool).await;

    get_payroll_run_detail(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();
    get_payroll_run_detail(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();

    assert_eq!(stored_status(&pool, &run_id).await, "draft");
    assert_eq!(action_log_entry_count(&pool).await, entries_before);
}

#[sqlx::test]
async fn another_employers_run_is_not_found(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let ours = an_employer(&db, "Ours").await;
    let theirs = an_employer(&db, "Theirs").await;
    let their_run = a_run(&db, &theirs, february_period()).await;

    let refusal = get_payroll_run_detail(&db, &ours, their_run.as_str())
        .await
        .unwrap_err();

    assert!(matches!(refusal, PayrollAppError::PayrollRunNotFound(id) if id == their_run));
}

/// An id that does not exist and an id belonging to another Employer are the
/// same refusal: 403 would confirm the id exists, which is a free existence
/// oracle over payroll (ADR-0017).
#[sqlx::test]
async fn an_unknown_run_id_is_the_same_refusal_as_another_employers(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db, "Employer").await;
    let unknown = "0199ffff-ffff-7fff-bfff-ffffffffffff";

    let refusal = get_payroll_run_detail(&db, &employer_id, unknown)
        .await
        .unwrap_err();

    assert!(matches!(refusal, PayrollAppError::PayrollRunNotFound(id) if id.as_str() == unknown));
}

/// A path segment that is not a UUID at all is the same 404, not the
/// database error the `::uuid` cast would otherwise raise — a client's typo
/// must never become a 500.
#[sqlx::test]
async fn a_run_id_that_is_not_a_uuid_is_not_found(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db, "Employer").await;

    let refusal = get_payroll_run_detail(&db, &employer_id, "does-not-exist")
        .await
        .unwrap_err();

    assert!(matches!(refusal, PayrollAppError::PayrollRunNotFound(_)));
}

// ---- VerifyPayrollRunBelongsToEmployer ----

#[sqlx::test]
async fn verifying_a_run_of_this_employer_hands_back_its_id(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db, "Employer").await;
    let run_id = a_run(&db, &employer_id, february_period()).await;

    let verified = verify_payroll_run_belongs_to_employer(&db, &employer_id, run_id.as_str())
        .await
        .unwrap();

    assert_eq!(verified, run_id);
}

#[sqlx::test]
async fn verifying_another_employers_run_is_not_found(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let ours = an_employer(&db, "Ours").await;
    let theirs = an_employer(&db, "Theirs").await;
    let their_run = a_run(&db, &theirs, february_period()).await;

    let refusal = verify_payroll_run_belongs_to_employer(&db, &ours, their_run.as_str())
        .await
        .unwrap_err();

    assert!(matches!(refusal, PayrollAppError::PayrollRunNotFound(id) if id == their_run));
}

#[sqlx::test]
async fn verifying_a_run_id_that_is_not_a_uuid_is_not_found(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let employer_id = an_employer(&db, "Employer").await;

    let refusal = verify_payroll_run_belongs_to_employer(&db, &employer_id, "does-not-exist")
        .await
        .unwrap_err();

    assert!(matches!(refusal, PayrollAppError::PayrollRunNotFound(_)));
}
