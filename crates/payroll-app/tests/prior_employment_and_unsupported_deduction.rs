//! Proves the use cases issue #27 introduces: `declare_prior_employment` and
//! `declare_unsupported_deduction_status` — the seams
//! `docs/domain/payroll-run-persistence.md` §4.5b and §4.5c describe,
//! reached through the public API a later ticket calls, not raw SQL.

use chrono::NaiveDate;
use payroll::{
    DayOfMonth, EmploymentId, Money, PayrollError, PeriodEndDay, PersonId, PriorEmployment,
    PriorEmploymentFigures, TaxYear, UnsupportedDeductionKind, UnsupportedDeductionKinds,
    UnsupportedDeductionStatus,
};
use payroll_app::{
    PayrollAppError, create_employer, create_employment, declare_prior_employment,
    declare_unsupported_deduction_status, void_employment,
};
use sqlx::{PgPool, Row};

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// A 26th-to-25th monthly schedule: 2026-01-26 and 2026-02-26 are period
/// starts and 2026-01-10 is not.
fn twenty_sixth_schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap()))
}

async fn an_employer_and_employment(pool: &PgPool) -> (payroll::EmployerId, EmploymentId) {
    let employer_id = create_employer(pool, twenty_sixth_schedule(), "actor")
        .await
        .unwrap();
    let employment_id = create_employment(
        pool,
        &employer_id,
        &PersonId::new("person-1"),
        date(2026, 1, 26),
        None,
        "actor",
    )
    .await
    .unwrap();
    (employer_id, employment_id)
}

// ---- PriorEmployment (§4.5b) ----

#[sqlx::test]
async fn a_confirmed_none_prior_employment_is_recorded_with_no_figures(pool: PgPool) {
    let (_, employment_id) = an_employer_and_employment(&pool).await;

    declare_prior_employment(
        &pool,
        &employment_id,
        TaxYear::starting(2026),
        PriorEmployment::None,
        "actor",
    )
    .await
    .unwrap();

    let row = sqlx::query(
        "SELECT status, taxable_remuneration::bigint, paye::bigint
         FROM prior_employment_declaration
         WHERE employment_id = $1 AND tax_year = 2026",
    )
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>(0), "confirmed_none");
    assert_eq!(row.get::<Option<i64>, _>(1), None);
    assert_eq!(row.get::<Option<i64>, _>(2), None);
}

#[sqlx::test]
async fn a_present_prior_employment_is_recorded_with_its_figures_set_together(pool: PgPool) {
    let (_, employment_id) = an_employer_and_employment(&pool).await;
    let figures = PriorEmploymentFigures::new(
        Money::from_cents(150_000).unwrap(),
        Money::from_cents(20_000).unwrap(),
    );

    declare_prior_employment(
        &pool,
        &employment_id,
        TaxYear::starting(2026),
        PriorEmployment::Some(figures),
        "actor",
    )
    .await
    .unwrap();

    let row = sqlx::query(
        "SELECT status, taxable_remuneration::bigint, paye::bigint
         FROM prior_employment_declaration
         WHERE employment_id = $1 AND tax_year = 2026",
    )
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>(0), "present");
    assert_eq!(row.get::<Option<i64>, _>(1), Some(150_000));
    assert_eq!(row.get::<Option<i64>, _>(2), Some(20_000));
}

#[sqlx::test]
async fn declaring_prior_employment_as_unknown_is_refused(pool: PgPool) {
    let (_, employment_id) = an_employer_and_employment(&pool).await;

    let result = declare_prior_employment(
        &pool,
        &employment_id,
        TaxYear::starting(2026),
        PriorEmployment::Unknown,
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::PriorEmploymentDeclarationCannotBeUnknown)
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM prior_employment_declaration")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "a refused Unknown declaration must not be written"
    );
}

#[sqlx::test]
async fn declaring_prior_employment_against_a_missing_employment_is_refused(pool: PgPool) {
    let missing = EmploymentId::new("does-not-exist");

    let result = declare_prior_employment(
        &pool,
        &missing,
        TaxYear::starting(2026),
        PriorEmployment::None,
        "actor",
    )
    .await;

    assert_eq!(result, Err(PayrollAppError::EmploymentNotFound(missing)));
}

#[sqlx::test]
async fn a_voided_employment_accepts_no_prior_employment_declaration(pool: PgPool) {
    let (_, employment_id) = an_employer_and_employment(&pool).await;
    void_employment(&pool, &employment_id, "actor")
        .await
        .unwrap();

    let result = declare_prior_employment(
        &pool,
        &employment_id,
        TaxYear::starting(2026),
        PriorEmployment::None,
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()))
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM prior_employment_declaration")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[sqlx::test]
async fn the_first_prior_employment_declaration_writes_a_declared_action_log_entry(pool: PgPool) {
    let (employer_id, employment_id) = an_employer_and_employment(&pool).await;

    declare_prior_employment(
        &pool,
        &employment_id,
        TaxYear::starting(2026),
        PriorEmployment::None,
        "actor",
    )
    .await
    .unwrap();

    let row = sqlx::query(
        "SELECT employer_id, actor, action_type, target_type, target_id
         FROM action_log_entry WHERE target_id = $1",
    )
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>(0), employer_id.as_str());
    assert_eq!(row.get::<String, _>(1), "actor");
    assert_eq!(row.get::<String, _>(2), "prior_employment_declared");
    assert_eq!(row.get::<String, _>(3), "employment");
}

/// The same (Employment, TaxYear) declared twice replaces the row rather
/// than duplicating it, and the second write is a `Changed` act, not a
/// second `Declared` — the pair the ActionType catalogue names for §4.5b.
#[sqlx::test]
async fn redeclaring_prior_employment_replaces_the_row_and_writes_a_changed_entry(pool: PgPool) {
    let (_, employment_id) = an_employer_and_employment(&pool).await;

    declare_prior_employment(
        &pool,
        &employment_id,
        TaxYear::starting(2026),
        PriorEmployment::None,
        "actor",
    )
    .await
    .unwrap();

    let figures = PriorEmploymentFigures::new(Money::from_cents(50_000).unwrap(), Money::ZERO);
    declare_prior_employment(
        &pool,
        &employment_id,
        TaxYear::starting(2026),
        PriorEmployment::Some(figures),
        "actor",
    )
    .await
    .unwrap();

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM prior_employment_declaration")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "one row per (Employment, TaxYear), not two");

    let row = sqlx::query(
        "SELECT status, taxable_remuneration::bigint FROM prior_employment_declaration
         WHERE employment_id = $1 AND tax_year = 2026",
    )
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>(0), "present");
    assert_eq!(row.get::<Option<i64>, _>(1), Some(50_000));

    let action_types: Vec<String> = sqlx::query_scalar(
        "SELECT action_type FROM action_log_entry WHERE target_id = $1 ORDER BY occurred_at",
    )
    .bind(employment_id.as_str())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        action_types,
        vec!["prior_employment_declared", "prior_employment_changed"]
    );
}

#[sqlx::test]
async fn an_unattributed_prior_employment_declaration_is_refused_by_the_database(pool: PgPool) {
    let (_, employment_id) = an_employer_and_employment(&pool).await;

    let result = declare_prior_employment(
        &pool,
        &employment_id,
        TaxYear::starting(2026),
        PriorEmployment::None,
        "",
    )
    .await;

    assert!(
        matches!(result, Err(PayrollAppError::Database(_))),
        "expected a Database refusal, got {result:?}"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM prior_employment_declaration")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

// ---- UnsupportedDeductionStatus (§4.5c) ----

#[sqlx::test]
async fn a_confirmed_none_unsupported_deduction_status_is_recorded_with_no_kinds(pool: PgPool) {
    let (_, employment_id) = an_employer_and_employment(&pool).await;

    declare_unsupported_deduction_status(
        &pool,
        &employment_id,
        date(2026, 1, 26),
        UnsupportedDeductionStatus::ConfirmedNone,
        "actor",
    )
    .await
    .unwrap();

    let row = sqlx::query(
        "SELECT status, kinds FROM unsupported_deduction_declaration
         WHERE employment_id = $1 AND effective_from = '2026-01-26'",
    )
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>(0), "confirmed_none");
    assert_eq!(row.get::<Option<serde_json::Value>, _>(1), None);
}

#[sqlx::test]
async fn a_present_unsupported_deduction_status_is_recorded_with_its_named_kinds(pool: PgPool) {
    let (_, employment_id) = an_employer_and_employment(&pool).await;
    let kinds = UnsupportedDeductionKinds::new(vec![
        UnsupportedDeductionKind::ProvidentFund,
        UnsupportedDeductionKind::EducationPolicy,
    ])
    .unwrap();

    declare_unsupported_deduction_status(
        &pool,
        &employment_id,
        date(2026, 1, 26),
        UnsupportedDeductionStatus::Present(kinds),
        "actor",
    )
    .await
    .unwrap();

    let row = sqlx::query(
        "SELECT status, kinds FROM unsupported_deduction_declaration
         WHERE employment_id = $1 AND effective_from = '2026-01-26'",
    )
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>(0), "present");
    let kinds: serde_json::Value = row.get(1);
    assert_eq!(
        kinds,
        serde_json::json!(["ProvidentFund", "EducationPolicy"])
    );
}

#[sqlx::test]
async fn an_effective_from_that_is_not_a_pay_period_start_is_a_domain_refusal(pool: PgPool) {
    let (_, employment_id) = an_employer_and_employment(&pool).await;

    let result = declare_unsupported_deduction_status(
        &pool,
        &employment_id,
        date(2026, 1, 10),
        UnsupportedDeductionStatus::ConfirmedNone,
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::from(
            PayrollError::EffectiveFromNotAPeriodStart {
                next_valid_effective_from: date(2026, 1, 26),
            }
        ))
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM unsupported_deduction_declaration")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0, "a refused declaration must not be written");
}

#[sqlx::test]
async fn declaring_unsupported_deduction_status_as_unknown_is_refused(pool: PgPool) {
    let (_, employment_id) = an_employer_and_employment(&pool).await;

    let result = declare_unsupported_deduction_status(
        &pool,
        &employment_id,
        date(2026, 1, 26),
        UnsupportedDeductionStatus::Unknown,
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::UnsupportedDeductionDeclarationCannotBeUnknown)
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM unsupported_deduction_declaration")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 0,
        "a refused Unknown declaration must not be written"
    );
}

#[sqlx::test]
async fn declaring_unsupported_deduction_status_against_a_missing_employment_is_refused(
    pool: PgPool,
) {
    let missing = EmploymentId::new("does-not-exist");

    let result = declare_unsupported_deduction_status(
        &pool,
        &missing,
        date(2026, 1, 26),
        UnsupportedDeductionStatus::ConfirmedNone,
        "actor",
    )
    .await;

    assert_eq!(result, Err(PayrollAppError::EmploymentNotFound(missing)));
}

#[sqlx::test]
async fn a_voided_employment_accepts_no_unsupported_deduction_status_declaration(pool: PgPool) {
    let (_, employment_id) = an_employer_and_employment(&pool).await;
    void_employment(&pool, &employment_id, "actor")
        .await
        .unwrap();

    let result = declare_unsupported_deduction_status(
        &pool,
        &employment_id,
        date(2026, 1, 26),
        UnsupportedDeductionStatus::ConfirmedNone,
        "actor",
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()))
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM unsupported_deduction_declaration")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

/// The story §4.5c gives for why this fact is effective-dated rather than
/// Employment+TaxYear state, reproduced directly: March is correctly
/// `ConfirmedNone`, and joining a provident fund partway through the year
/// is a second, later-effective row — never a rewrite of March.
#[sqlx::test]
async fn a_later_effective_from_adds_a_second_row_rather_than_rewriting_the_first(pool: PgPool) {
    let (_, employment_id) = an_employer_and_employment(&pool).await;

    declare_unsupported_deduction_status(
        &pool,
        &employment_id,
        date(2026, 1, 26),
        UnsupportedDeductionStatus::ConfirmedNone,
        "actor",
    )
    .await
    .unwrap();

    let kinds =
        UnsupportedDeductionKinds::new(vec![UnsupportedDeductionKind::ProvidentFund]).unwrap();
    declare_unsupported_deduction_status(
        &pool,
        &employment_id,
        date(2026, 2, 26),
        UnsupportedDeductionStatus::Present(kinds),
        "actor",
    )
    .await
    .unwrap();

    let rows: Vec<(NaiveDate, String)> = sqlx::query_as(
        "SELECT effective_from, status FROM unsupported_deduction_declaration
         WHERE employment_id = $1 ORDER BY effective_from",
    )
    .bind(employment_id.as_str())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            (date(2026, 1, 26), "confirmed_none".to_string()),
            (date(2026, 2, 26), "present".to_string()),
        ]
    );
}

/// Re-declaring the exact same `effective_from` replaces that one row
/// rather than duplicating it, and every write against it — first or
/// later — logs as `UnsupportedDeductionStatusCorrected`: the ActionType
/// catalogue names only one act here, unlike §4.5b's Declared/Changed pair.
#[sqlx::test]
async fn redeclaring_the_same_effective_from_replaces_the_row_and_logs_a_correction(pool: PgPool) {
    let (_, employment_id) = an_employer_and_employment(&pool).await;

    declare_unsupported_deduction_status(
        &pool,
        &employment_id,
        date(2026, 1, 26),
        UnsupportedDeductionStatus::ConfirmedNone,
        "actor",
    )
    .await
    .unwrap();

    let kinds =
        UnsupportedDeductionKinds::new(vec![UnsupportedDeductionKind::RetirementAnnuityFund])
            .unwrap();
    declare_unsupported_deduction_status(
        &pool,
        &employment_id,
        date(2026, 1, 26),
        UnsupportedDeductionStatus::Present(kinds),
        "actor",
    )
    .await
    .unwrap();

    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM unsupported_deduction_declaration")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 1,
        "one row per (Employment, effective_from), not two"
    );

    let row = sqlx::query(
        "SELECT status FROM unsupported_deduction_declaration
         WHERE employment_id = $1 AND effective_from = '2026-01-26'",
    )
    .bind(employment_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<String, _>(0), "present");

    let action_types: Vec<String> = sqlx::query_scalar(
        "SELECT action_type FROM action_log_entry WHERE target_id = $1 ORDER BY occurred_at",
    )
    .bind(employment_id.as_str())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        action_types,
        vec![
            "unsupported_deduction_status_corrected",
            "unsupported_deduction_status_corrected"
        ]
    );
}

#[sqlx::test]
async fn an_unattributed_unsupported_deduction_declaration_is_refused_by_the_database(
    pool: PgPool,
) {
    let (_, employment_id) = an_employer_and_employment(&pool).await;

    let result = declare_unsupported_deduction_status(
        &pool,
        &employment_id,
        date(2026, 1, 26),
        UnsupportedDeductionStatus::ConfirmedNone,
        "",
    )
    .await;

    assert!(
        matches!(result, Err(PayrollAppError::Database(_))),
        "expected a Database refusal, got {result:?}"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM unsupported_deduction_declaration")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
}
