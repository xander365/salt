//! Proves the use cases issue #29 introduces: `create_ordinary_payroll_run`,
//! `remove_employment_from_run` and `set_run_pay_lines` — the Ordinary half
//! of `docs/domain/payroll-run-persistence.md` §4.6-§4.8 and §4.5d, reached
//! through the public API a later ticket calls, not raw SQL.

use chrono::NaiveDate;
use payroll::{DayOfMonth, EarningInstruction, EmploymentId, Money, PayPeriod, PeriodEndDay};
use payroll_app::{
    EmploymentPerson, PayLineInstruction, PayrollAppError, PayrollRunId, SaltDatabase,
    create_employer, create_employment, create_ordinary_payroll_run, remove_employment_from_run,
    set_run_pay_lines, void_employment,
};
use sqlx::{PgPool, Row};
use tokio::sync::oneshot;

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// A 26th-to-25th monthly schedule, matching the other tests in this crate.
fn twenty_sixth_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap()))
}

/// 2026-01-26 to 2026-02-25, one of `twenty_sixth_schedule()`'s own periods.
fn march_period() -> PayPeriod {
    PayPeriod::new(date(2026, 1, 26), date(2026, 2, 25)).unwrap()
}

fn allowance(cents: i64) -> EarningInstruction {
    EarningInstruction::TaxableAllowance {
        amount: Money::from_cents(cents).unwrap(),
        label: None,
    }
}

async fn an_employer(db: &SaltDatabase) -> payroll::EmployerId {
    create_employer(db, "Employer", twenty_sixth_schedule(), "actor")
        .await
        .unwrap()
}

async fn an_employment(
    db: &SaltDatabase,
    employer_id: &payroll::EmployerId,
    person: &str,
    start_date: NaiveDate,
    end_date: Option<NaiveDate>,
) -> EmploymentId {
    create_employment(
        db,
        employer_id,
        EmploymentPerson::New(person.to_string()),
        start_date,
        end_date,
        "actor",
    )
    .await
    .unwrap()
    .1
}

// ---- CreateOrdinaryPayrollRun (§4.6-§4.8) ----

#[sqlx::test]
async fn a_draft_ordinary_run_is_created_with_its_period_and_pay_date(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;

    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();

    let row = sqlx::query(
        "SELECT employer_id, period_start, period_end, pay_date, kind, status, correction_reason
         FROM payroll_run WHERE id = $1::uuid",
    )
    .bind(run_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>(0), employer_id.as_str());
    assert_eq!(row.get::<NaiveDate, _>(1), date(2026, 1, 26));
    assert_eq!(row.get::<NaiveDate, _>(2), date(2026, 2, 25));
    assert_eq!(row.get::<NaiveDate, _>(3), date(2026, 3, 1));
    assert_eq!(row.get::<String, _>(4), "ordinary");
    assert_eq!(row.get::<String, _>(5), "draft");
    assert_eq!(row.get::<Option<String>, _>(6), None);
}

#[sqlx::test]
async fn every_employment_overlapping_the_period_is_proposed(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let continuing = an_employment(&db, &employer_id, "person-1", date(2025, 1, 1), None).await;
    let joiner = an_employment(&db, &employer_id, "person-2", date(2026, 2, 1), None).await;
    let leaver = an_employment(
        &db,
        &employer_id,
        "person-3",
        date(2025, 1, 1),
        Some(date(2026, 1, 26)),
    )
    .await;

    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();

    let members: Vec<String> = sqlx::query_scalar(
        "SELECT employment_id FROM payroll_run_employment
         WHERE payroll_run_id = $1::uuid ORDER BY employment_id",
    )
    .bind(run_id.as_str())
    .fetch_all(&pool)
    .await
    .unwrap();
    let mut expected = vec![
        continuing.as_str().to_string(),
        joiner.as_str().to_string(),
        leaver.as_str().to_string(),
    ];
    expected.sort();
    assert_eq!(members, expected);
}

#[sqlx::test]
async fn an_employment_wholly_outside_the_period_is_not_proposed(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    an_employment(
        &db,
        &employer_id,
        "person-1",
        date(2024, 1, 1),
        Some(date(2026, 1, 25)),
    )
    .await;
    an_employment(&db, &employer_id, "person-2", date(2026, 2, 26), None).await;

    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payroll_run_employment WHERE payroll_run_id = $1::uuid",
    )
    .bind(run_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test]
async fn a_voided_employment_is_never_proposed(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    let voided = an_employment(&db, &employer_id, "person-1", date(2025, 1, 1), None).await;
    void_employment(&db, &voided, "actor").await.unwrap();
    let live = an_employment(&db, &employer_id, "person-2", date(2025, 1, 1), None).await;

    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();

    let members: Vec<String> = sqlx::query_scalar(
        "SELECT employment_id FROM payroll_run_employment WHERE payroll_run_id = $1::uuid",
    )
    .bind(run_id.as_str())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(members, vec![live.as_str().to_string()]);
}

#[sqlx::test]
async fn an_employment_committing_during_run_creation_is_proposed(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;

    // The raw insert below needs a real `person` row: `employment.person_id`
    // carries a foreign key to `person` (issue #51).
    sqlx::query(
        "INSERT INTO person (id, employer_id, full_name, created_by)
         VALUES ('person-concurrently', $1, 'Test Person', 'actor')",
    )
    .bind(employer_id.as_str())
    .execute(&pool)
    .await
    .unwrap();

    let mut employment_transaction = pool.begin().await.unwrap();

    // This is the KEY SHARE lock create_employment's employer foreign key
    // takes. CreateOrdinaryPayrollRun must wait on it before taking its
    // membership snapshot, or this Employment can commit just after that
    // snapshot and be silently omitted.
    sqlx::query("SELECT id FROM employer WHERE id = $1 FOR KEY SHARE")
        .bind(employer_id.as_str())
        .fetch_one(&mut *employment_transaction)
        .await
        .unwrap();

    let (started_sender, started_receiver) = oneshot::channel();
    let creating_db = SaltDatabase::from_pool(pool.clone());
    let creating_employer = employer_id.clone();
    let creating_run = tokio::spawn(async move {
        started_sender.send(()).unwrap();
        create_ordinary_payroll_run(
            &creating_db,
            &creating_employer,
            march_period(),
            date(2026, 3, 1),
            "actor",
        )
        .await
    });
    started_receiver.await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        !creating_run.is_finished(),
        "run creation must wait for an Employment creation holding the Employer lock"
    );

    sqlx::query(
        "INSERT INTO employment (id, employer_id, person_id, start_date, end_date, created_by)
         VALUES ('employment-created-concurrently', $1, 'person-concurrently',
                 '2025-01-01', NULL, 'actor')",
    )
    .bind(employer_id.as_str())
    .execute(&mut *employment_transaction)
    .await
    .unwrap();
    employment_transaction.commit().await.unwrap();

    let run_id = creating_run.await.unwrap().unwrap();
    let members: Vec<String> = sqlx::query_scalar(
        "SELECT employment_id FROM payroll_run_employment WHERE payroll_run_id = $1::uuid",
    )
    .bind(run_id.as_str())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(members, vec!["employment-created-concurrently"]);
}

#[sqlx::test]
async fn a_second_ordinary_run_for_the_same_employer_and_period_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 1), "actor")
        .await
        .unwrap();

    let result =
        create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 5), "actor")
            .await;

    assert!(
        matches!(result, Err(PayrollAppError::Database(_))),
        "expected a Database refusal, got {result:?}"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM payroll_run")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[sqlx::test]
async fn creating_a_run_against_a_missing_employer_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let missing = payroll::EmployerId::new("does-not-exist");

    let result =
        create_ordinary_payroll_run(&db, &missing, march_period(), date(2026, 3, 1), "actor").await;

    assert_eq!(result, Err(PayrollAppError::EmployerNotFound(missing)));
}

#[sqlx::test]
async fn a_period_the_employers_schedule_does_not_generate_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    // Calendar February, under a schedule whose periods run the 26th to the
    // 25th. Nothing later could walk back from it to a preceding period.
    let calendar_february = PayPeriod::new(date(2026, 2, 1), date(2026, 2, 28)).unwrap();

    let result = create_ordinary_payroll_run(
        &db,
        &employer_id,
        calendar_february,
        date(2026, 3, 1),
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::PayPeriodNotGeneratedByThePaySchedule {
            period: calendar_february,
            schedules_period: PayPeriod::new(date(2026, 2, 26), date(2026, 3, 25)).unwrap(),
        })
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM payroll_run")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "a refused run must not be written");
}

#[sqlx::test]
async fn a_period_ending_on_a_schedule_boundary_but_starting_elsewhere_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;
    // The end date the Ordinary uniqueness index keys on is correct, so only
    // checking that half would let this through — priced over a span nobody
    // works.
    let short_period = PayPeriod::new(date(2026, 2, 1), date(2026, 2, 25)).unwrap();

    let result =
        create_ordinary_payroll_run(&db, &employer_id, short_period, date(2026, 3, 1), "actor")
            .await;

    assert_eq!(
        result,
        Err(PayrollAppError::PayPeriodNotGeneratedByThePaySchedule {
            period: short_period,
            schedules_period: march_period(),
        })
    );
}

#[sqlx::test]
async fn creating_a_run_writes_a_payroll_run_created_action_log_entry(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let employer_id = an_employer(&db).await;

    let run_id =
        create_ordinary_payroll_run(&db, &employer_id, march_period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();

    let row = sqlx::query(
        "SELECT employer_id, actor, action_type, target_type, target_id
         FROM action_log_entry WHERE target_id = $1",
    )
    .bind(run_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>(0), employer_id.as_str());
    assert_eq!(row.get::<String, _>(1), "actor");
    assert_eq!(row.get::<String, _>(2), "payroll_run_created");
    assert_eq!(row.get::<String, _>(3), "payroll_run");
}

// ---- RemoveEmploymentFromRun (§4.8) ----

async fn a_run_with_one_member(
    db: &SaltDatabase,
) -> (payroll::EmployerId, PayrollRunId, EmploymentId) {
    let employer_id = an_employer(db).await;
    let employment_id = an_employment(db, &employer_id, "person-1", date(2025, 1, 1), None).await;
    let run_id =
        create_ordinary_payroll_run(db, &employer_id, march_period(), date(2026, 3, 1), "actor")
            .await
            .unwrap();
    (employer_id, run_id, employment_id)
}

#[sqlx::test]
async fn removing_a_member_records_the_reason_actor_and_time(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, run_id, employment_id) = a_run_with_one_member(&db).await;

    remove_employment_from_run(&db, &run_id, &employment_id, "on unpaid leave", "reviewer")
        .await
        .unwrap();

    let row = sqlx::query(
        "SELECT removed_by, removal_reason, removed_at FROM payroll_run_employment
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>(0), "reviewer");
    assert_eq!(row.get::<String, _>(1), "on unpaid leave");
    assert!(
        row.get::<Option<chrono::DateTime<chrono::Utc>>, _>(2)
            .is_some()
    );
}

#[sqlx::test]
async fn removing_a_member_with_a_blank_reason_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    // A reason of `" "` states nothing while looking like it states
    // something, and the ActionLog it would land in cannot be corrected.
    for blank in ["", " ", "\t\n  "] {
        let (_, run_id, employment_id) = a_run_with_one_member(&db).await;

        let result =
            remove_employment_from_run(&db, &run_id, &employment_id, blank, "reviewer").await;

        assert_eq!(
            result,
            Err(PayrollAppError::RemovalReasonCannotBeEmpty),
            "a reason of {blank:?} must be refused"
        );
        let removed_at: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
            "SELECT removed_at FROM payroll_run_employment
             WHERE payroll_run_id = $1::uuid AND employment_id = $2",
        )
        .bind(run_id.as_str())
        .bind(employment_id.as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(removed_at, None);
    }
}

#[sqlx::test]
async fn removing_an_employment_that_is_not_a_member_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (employer_id, run_id, _) = a_run_with_one_member(&db).await;
    let outsider = an_employment(&db, &employer_id, "person-2", date(2026, 2, 26), None).await;

    let result =
        remove_employment_from_run(&db, &run_id, &outsider, "wrong person", "reviewer").await;

    assert_eq!(
        result,
        Err(PayrollAppError::EmploymentNotAnActiveRunMember {
            payroll_run_id: run_id,
            employment_id: outsider,
        })
    );
}

#[sqlx::test]
async fn removing_an_already_removed_member_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, run_id, employment_id) = a_run_with_one_member(&db).await;
    remove_employment_from_run(&db, &run_id, &employment_id, "first reason", "reviewer")
        .await
        .unwrap();

    let result = remove_employment_from_run(
        &db,
        &run_id,
        &employment_id,
        "second reason",
        "someone-else",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::EmploymentNotAnActiveRunMember {
            payroll_run_id: run_id.clone(),
            employment_id: employment_id.clone(),
        })
    );
    let row = sqlx::query(
        "SELECT removed_by, removal_reason FROM payroll_run_employment
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        row.get::<String, _>(0),
        "reviewer",
        "a refused second removal must not overwrite who removed it first"
    );
    assert_eq!(row.get::<String, _>(1), "first reason");
}

/// Seeing the calculated figures is exactly when an Employer notices that
/// someone should not be paid this period, so a `Calculated` run must still
/// accept a removal — and the removal reopens it. The stored calculations
/// are no longer a current account of the run's members the moment one
/// leaves, which is the definition of `Draft` (§4.7).
#[sqlx::test]
async fn removing_a_member_from_a_calculated_run_reopens_it(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, run_id, employment_id) = a_run_with_one_member(&db).await;
    sqlx::query("UPDATE payroll_run SET status = 'calculated' WHERE id = $1::uuid")
        .bind(run_id.as_str())
        .execute(&pool)
        .await
        .unwrap();

    remove_employment_from_run(&db, &run_id, &employment_id, "unpaid leave", "actor")
        .await
        .unwrap();

    let status: String = sqlx::query_scalar("SELECT status FROM payroll_run WHERE id = $1::uuid")
        .bind(run_id.as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "draft");
}

/// `Finalized` is the one absolute refusal: history has been written, and
/// working state can no longer change (§4.7).
#[sqlx::test]
async fn a_member_cannot_be_removed_after_finalization(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, run_id, employment_id) = a_run_with_one_member(&db).await;
    sqlx::query("UPDATE payroll_run SET status = 'finalized' WHERE id = $1::uuid")
        .bind(run_id.as_str())
        .execute(&pool)
        .await
        .unwrap();

    let result =
        remove_employment_from_run(&db, &run_id, &employment_id, "unpaid leave", "actor").await;

    assert_eq!(
        result,
        Err(PayrollAppError::PayrollRunAlreadyFinalized {
            payroll_run_id: run_id,
            finalized_payrolls: Vec::new(),
        })
    );
}

#[sqlx::test]
async fn removing_a_member_writes_an_employment_removed_from_run_entry_carrying_its_reason(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (employer_id, run_id, employment_id) = a_run_with_one_member(&db).await;

    remove_employment_from_run(&db, &run_id, &employment_id, "on unpaid leave", "reviewer")
        .await
        .unwrap();

    let row = sqlx::query(
        "SELECT employer_id, actor, action_type, target_type, target_id, context
         FROM action_log_entry WHERE target_id = $1",
    )
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>(0), employer_id.as_str());
    assert_eq!(row.get::<String, _>(1), "reviewer");
    assert_eq!(row.get::<String, _>(2), "employment_removed_from_run");
    assert_eq!(row.get::<String, _>(3), "employment");
    let context: serde_json::Value = row.get(5);
    assert_eq!(context, serde_json::json!({ "reason": "on unpaid leave" }));
}

// ---- SetRunPayLines (§4.5d, issue #77) ----

#[sqlx::test]
async fn earning_lines_are_stored_in_the_order_given(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, run_id, employment_id) = a_run_with_one_member(&db).await;
    let earnings = vec![allowance(50_000), allowance(10_000)];

    set_run_pay_lines(&db, &run_id, &employment_id, earnings.clone(), Vec::new())
        .await
        .unwrap();

    let rows: Vec<(i16, serde_json::Value)> = sqlx::query_as(
        "SELECT line, pay_line_json FROM payroll_run_pay_line
         WHERE payroll_run_id = $1::uuid AND employment_id = $2 ORDER BY line",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, 0);
    assert_eq!(rows[1].0, 1);
    assert_eq!(
        serde_json::from_value::<PayLineInstruction>(rows[0].1.clone()).unwrap(),
        PayLineInstruction::Earning(earnings[0].clone())
    );
    assert_eq!(
        serde_json::from_value::<PayLineInstruction>(rows[1].1.clone()).unwrap(),
        PayLineInstruction::Earning(earnings[1].clone())
    );
}

/// A line typed directly onto a run is `one_off` (§0's own CONTEXT.md entry
/// for `StandingPayItem`): never a StandingPayItem, and never recurring.
#[sqlx::test]
async fn earning_lines_written_through_set_run_pay_lines_are_sourced_one_off(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, run_id, employment_id) = a_run_with_one_member(&db).await;

    set_run_pay_lines(
        &db,
        &run_id,
        &employment_id,
        vec![allowance(50_000)],
        Vec::new(),
    )
    .await
    .unwrap();

    let source: String = sqlx::query_scalar(
        "SELECT source FROM payroll_run_pay_line
         WHERE payroll_run_id = $1::uuid AND employment_id = $2 AND line = 0",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(source, "one_off");
}

/// The real defect issue #77 closes: today's write left a member's
/// `WorkingPayrollCalculation` in place, so the run detail's join could show
/// figures older than the inputs beside them. A write that changes the lines
/// now deletes it in the same transaction (`pay_line_staleness.rs` proves
/// the clearing and resubmitting cases through real calculations).
#[sqlx::test]
async fn writing_pay_lines_deletes_the_members_stale_working_calculation(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, run_id, employment_id) = a_run_with_one_member(&db).await;
    sqlx::query(
        "INSERT INTO working_payroll_calculation
            (payroll_run_id, employment_id, payroll_input_json, payroll_rules_json,
             payroll_calculation_json, calculated_by)
         VALUES ($1::uuid, $2, '{}', '{}', '{}', 'calculator')",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .execute(&pool)
    .await
    .unwrap();

    set_run_pay_lines(
        &db,
        &run_id,
        &employment_id,
        vec![allowance(10_000)],
        Vec::new(),
    )
    .await
    .unwrap();

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM working_payroll_calculation
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count, 0,
        "a write must delete the stale WorkingCalculation in the same transaction"
    );
}

#[sqlx::test]
async fn no_earning_lines_is_a_complete_statement_of_no_additional_earnings(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, run_id, employment_id) = a_run_with_one_member(&db).await;

    set_run_pay_lines(&db, &run_id, &employment_id, Vec::new(), Vec::new())
        .await
        .unwrap();

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payroll_run_pay_line
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test]
async fn setting_earnings_again_replaces_rather_than_appends(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, run_id, employment_id) = a_run_with_one_member(&db).await;
    set_run_pay_lines(
        &db,
        &run_id,
        &employment_id,
        vec![allowance(50_000)],
        Vec::new(),
    )
    .await
    .unwrap();

    set_run_pay_lines(&db, &run_id, &employment_id, Vec::new(), Vec::new())
        .await
        .unwrap();

    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payroll_run_pay_line
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count, 0,
        "the second call must clear the first call's lines"
    );
}

/// An Earning corrected after the figures are on screen is the ordinary
/// case, not an exception: the edit is accepted and the run reopens, so its
/// status stops claiming calculations that the new line has made stale.
#[sqlx::test]
async fn changing_earnings_on_a_calculated_run_reopens_it(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, run_id, employment_id) = a_run_with_one_member(&db).await;
    sqlx::query("UPDATE payroll_run SET status = 'calculated' WHERE id = $1::uuid")
        .bind(run_id.as_str())
        .execute(&pool)
        .await
        .unwrap();

    set_run_pay_lines(
        &db,
        &run_id,
        &employment_id,
        vec![allowance(10_000)],
        Vec::new(),
    )
    .await
    .unwrap();

    let status: String = sqlx::query_scalar("SELECT status FROM payroll_run WHERE id = $1::uuid")
        .bind(run_id.as_str())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(status, "draft");
}

#[sqlx::test]
async fn earnings_cannot_change_after_finalization(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (_, run_id, employment_id) = a_run_with_one_member(&db).await;
    sqlx::query("UPDATE payroll_run SET status = 'finalized' WHERE id = $1::uuid")
        .bind(run_id.as_str())
        .execute(&pool)
        .await
        .unwrap();

    let result = set_run_pay_lines(
        &db,
        &run_id,
        &employment_id,
        vec![allowance(10_000)],
        Vec::new(),
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::PayrollRunAlreadyFinalized {
            payroll_run_id: run_id,
            finalized_payrolls: Vec::new(),
        })
    );
}

#[sqlx::test]
async fn setting_earnings_for_an_employment_that_is_not_a_run_member_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    let (employer_id, run_id, _) = a_run_with_one_member(&db).await;
    let outsider = an_employment(&db, &employer_id, "person-2", date(2026, 2, 26), None).await;

    let result =
        set_run_pay_lines(&db, &run_id, &outsider, vec![allowance(10_000)], Vec::new()).await;

    assert_eq!(
        result,
        Err(PayrollAppError::EmploymentNotAnActiveRunMember {
            payroll_run_id: run_id,
            employment_id: outsider,
        })
    );
}

#[sqlx::test]
async fn setting_earnings_for_a_removed_member_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool.clone());
    // A removal is the Employer's deliberate statement that this person is
    // not paid this period. Lines written afterwards would sit in the run
    // looking like pay that was intended.
    let (_, run_id, employment_id) = a_run_with_one_member(&db).await;
    remove_employment_from_run(&db, &run_id, &employment_id, "on unpaid leave", "reviewer")
        .await
        .unwrap();

    let result = set_run_pay_lines(
        &db,
        &run_id,
        &employment_id,
        vec![allowance(10_000)],
        Vec::new(),
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::EmploymentNotAnActiveRunMember {
            payroll_run_id: run_id.clone(),
            employment_id: employment_id.clone(),
        })
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM payroll_run_pay_line
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
}
