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
    declare_unsupported_deduction_status, get_prior_employment, get_unsupported_deduction_status,
    void_employment,
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
        "SELECT status, taxable_remuneration, paye
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
        "SELECT status, taxable_remuneration, paye
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
        "SELECT status, taxable_remuneration FROM prior_employment_declaration
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
        &[],
        "a reason",
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
        &[],
        "a reason",
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
        &[],
        "a reason",
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
        &[],
        "a reason",
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
        &[],
        "a reason",
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
        &[],
        "a reason",
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
        &[],
        "a reason",
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
        &[],
        "a reason",
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
        &[],
        "a reason",
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
        &[],
        "a reason",
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
        &[],
        "a reason",
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

// ---- Reading the declarations back (§4.5b, §4.5c) ----

/// The criterion the whole three-valued design rests on: silence reads as
/// `Unknown`, never as a confirmed none.
#[sqlx::test]
async fn no_prior_employment_row_reads_back_as_unknown(pool: PgPool) {
    let (_, employment_id) = an_employer_and_employment(&pool).await;

    let prior_employment = get_prior_employment(&pool, &employment_id, TaxYear::starting(2026))
        .await
        .unwrap();

    assert_eq!(prior_employment, PriorEmployment::Unknown);
}

/// A declaration for one TaxYear says nothing about another, so the year
/// with no row of its own is still `Unknown` — "nothing else means
/// Unknown" cuts both ways.
#[sqlx::test]
async fn a_declaration_for_one_tax_year_leaves_another_unknown(pool: PgPool) {
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

    let declared = get_prior_employment(&pool, &employment_id, TaxYear::starting(2026))
        .await
        .unwrap();
    let other_year = get_prior_employment(&pool, &employment_id, TaxYear::starting(2027))
        .await
        .unwrap();

    assert_eq!(declared, PriorEmployment::None);
    assert_eq!(other_year, PriorEmployment::Unknown);
}

#[sqlx::test]
async fn a_present_prior_employment_reads_back_with_both_figures(pool: PgPool) {
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

    let prior_employment = get_prior_employment(&pool, &employment_id, TaxYear::starting(2026))
        .await
        .unwrap();

    assert_eq!(prior_employment, PriorEmployment::Some(figures));
}

/// A caller naming an Employment that does not exist is not silence, and
/// answering `Unknown` would turn a mistyped id into a fact about payroll.
#[sqlx::test]
async fn reading_prior_employment_for_a_missing_employment_is_refused(pool: PgPool) {
    let missing = EmploymentId::new("no-such-employment");

    let result = get_prior_employment(&pool, &missing, TaxYear::starting(2026)).await;

    assert_eq!(result, Err(PayrollAppError::EmploymentNotFound(missing)));
}

#[sqlx::test]
async fn reading_prior_employment_for_a_voided_employment_is_refused(pool: PgPool) {
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
    void_employment(&pool, &employment_id, "actor")
        .await
        .unwrap();

    let result = get_prior_employment(&pool, &employment_id, TaxYear::starting(2026)).await;

    assert_eq!(
        result,
        Err(PayrollAppError::EmploymentIsVoid(employment_id))
    );
}

#[sqlx::test]
async fn no_unsupported_deduction_row_in_force_reads_back_as_unknown(pool: PgPool) {
    let (_, employment_id) = an_employer_and_employment(&pool).await;

    let status = get_unsupported_deduction_status(&pool, &employment_id, date(2026, 2, 25))
        .await
        .unwrap();

    assert_eq!(status, UnsupportedDeductionStatus::Unknown);
}

/// §4.5c's own story, read from the other side: exactly one row governs
/// any given period, and it is the latest one effective on or before that
/// period's end. March through July stay `ConfirmedNone` while August
/// onwards is `Present`, and the period before the first declaration is
/// still `Unknown`.
#[sqlx::test]
async fn the_latest_row_effective_on_or_before_the_period_end_governs_it(pool: PgPool) {
    let (_, employment_id) = an_employer_and_employment(&pool).await;
    declare_unsupported_deduction_status(
        &pool,
        &employment_id,
        date(2026, 2, 26),
        UnsupportedDeductionStatus::ConfirmedNone,
        &[],
        "a reason",
        "actor",
    )
    .await
    .unwrap();
    let kinds =
        UnsupportedDeductionKinds::new(vec![UnsupportedDeductionKind::ProvidentFund]).unwrap();
    declare_unsupported_deduction_status(
        &pool,
        &employment_id,
        date(2026, 7, 26),
        UnsupportedDeductionStatus::Present(kinds.clone()),
        &[],
        "a reason",
        "actor",
    )
    .await
    .unwrap();

    let before_any = get_unsupported_deduction_status(&pool, &employment_id, date(2026, 2, 25))
        .await
        .unwrap();
    let governed_by_the_first =
        get_unsupported_deduction_status(&pool, &employment_id, date(2026, 6, 25))
            .await
            .unwrap();
    let governed_by_the_second =
        get_unsupported_deduction_status(&pool, &employment_id, date(2026, 8, 25))
            .await
            .unwrap();

    assert_eq!(before_any, UnsupportedDeductionStatus::Unknown);
    assert_eq!(
        governed_by_the_first,
        UnsupportedDeductionStatus::ConfirmedNone
    );
    assert_eq!(
        governed_by_the_second,
        UnsupportedDeductionStatus::Present(kinds)
    );
}

#[sqlx::test]
async fn reading_unsupported_deduction_status_for_a_missing_employment_is_refused(pool: PgPool) {
    let missing = EmploymentId::new("no-such-employment");

    let result = get_unsupported_deduction_status(&pool, &missing, date(2026, 2, 25)).await;

    assert_eq!(result, Err(PayrollAppError::EmploymentNotFound(missing)));
}

#[sqlx::test]
async fn reading_unsupported_deduction_status_for_a_voided_employment_is_refused(pool: PgPool) {
    let (_, employment_id) = an_employer_and_employment(&pool).await;
    declare_unsupported_deduction_status(
        &pool,
        &employment_id,
        date(2026, 1, 26),
        UnsupportedDeductionStatus::ConfirmedNone,
        &[],
        "a reason",
        "actor",
    )
    .await
    .unwrap();
    void_employment(&pool, &employment_id, "actor")
        .await
        .unwrap();

    let result = get_unsupported_deduction_status(&pool, &employment_id, date(2026, 2, 25)).await;

    assert_eq!(
        result,
        Err(PayrollAppError::EmploymentIsVoid(employment_id))
    );
}

// ---- What the schema refuses on its own (0020) ----

/// `PriorEmploymentFigures` holds two `Money` amounts and a `Money` is
/// never negative, so the reader reconstructs one with `Money::from_cents`
/// and can only `expect` it to succeed. The CHECK is what makes that
/// expectation a schema guarantee rather than a hope about every writer.
#[sqlx::test]
async fn the_database_refuses_a_negative_prior_employment_figure(pool: PgPool) {
    let (_, employment_id) = an_employer_and_employment(&pool).await;

    let result = sqlx::query(
        "INSERT INTO prior_employment_declaration
            (employment_id, tax_year, status, taxable_remuneration, paye, declared_by)
         VALUES ($1, 2026, 'present', -1, 20000, 'actor')",
    )
    .bind(employment_id.as_str())
    .execute(&pool)
    .await;

    assert!(
        result.is_err(),
        "expected the CHECK to refuse a negative Money"
    );
}

/// The kinds column is the serialized form of a non-empty JSON array, and
/// anything else is refused rather than aborting the CHECK's own
/// evaluation.
#[sqlx::test]
async fn the_database_refuses_kinds_that_are_not_a_non_empty_array(pool: PgPool) {
    let (_, employment_id) = an_employer_and_employment(&pool).await;

    for kinds in [r#"{"ProvidentFund": true}"#, "[]", r#""ProvidentFund""#] {
        let result = sqlx::query(
            "INSERT INTO unsupported_deduction_declaration
                (employment_id, effective_from, status, kinds, declared_by)
             VALUES ($1, DATE '2026-01-26', 'present', $2::jsonb, 'actor')",
        )
        .bind(employment_id.as_str())
        .bind(kinds)
        .execute(&pool)
        .await;

        assert!(
            result.is_err(),
            "expected the CHECK to refuse kinds {kinds}"
        );
    }
}
