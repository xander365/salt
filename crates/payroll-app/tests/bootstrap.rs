//! Proves `bootstrap` (issue #48): the first Operator, the first Employer
//! and the Owner `EmployerMembership` binding them, all in one transaction,
//! reached only through the public API a later ticket's CLI calls.

use payroll_app::{
    BootstrapPeriodEndDay, MembershipRole, PayrollAppError, SaltDatabase, active_membership_role,
    bootstrap, create_operator, find_operator_by_email, list_employers_for_operator,
};
use sqlx::PgPool;

#[sqlx::test]
async fn bootstrap_creates_the_operator_employer_and_owner_membership(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);

    let outcome = bootstrap(
        &db,
        "alice@example.com",
        "Alice",
        "correct horse battery staple",
        "Acme Corp",
        BootstrapPeriodEndDay::Day(25),
    )
    .await
    .unwrap();

    let operator = find_operator_by_email(&db, "alice@example.com")
        .await
        .unwrap()
        .expect("bootstrap must create an Operator findable by its own email");
    assert_eq!(operator.id, outcome.operator_id);
    assert_eq!(operator.display_name, "Alice");

    let employers = list_employers_for_operator(&db, &outcome.operator_id)
        .await
        .unwrap();
    assert_eq!(employers.len(), 1);
    assert_eq!(employers[0].id, outcome.employer_id);
    assert_eq!(employers[0].name, "Acme Corp");
    assert_eq!(employers[0].role, MembershipRole::Owner);

    let role = active_membership_role(&db, &outcome.operator_id, &outcome.employer_id)
        .await
        .unwrap();
    assert_eq!(role, Some(MembershipRole::Owner));
}

#[sqlx::test]
async fn bootstrap_accepts_the_last_day_of_month_schedule(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);

    let outcome = bootstrap(
        &db,
        "alice@example.com",
        "Alice",
        "correct horse battery staple",
        "Acme Corp",
        BootstrapPeriodEndDay::LastDayOfMonth,
    )
    .await
    .unwrap();

    let employers = list_employers_for_operator(&db, &outcome.operator_id)
        .await
        .unwrap();
    assert_eq!(employers.len(), 1);
}

#[sqlx::test]
async fn bootstrap_refuses_when_an_operator_already_exists(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    create_operator(
        &db,
        "existing@example.com",
        "Existing Operator",
        "correct horse battery staple",
    )
    .await
    .unwrap();

    let result = bootstrap(
        &db,
        "alice@example.com",
        "Alice",
        "correct horse battery staple",
        "Acme Corp",
        BootstrapPeriodEndDay::Day(25),
    )
    .await;

    assert_eq!(result, Err(PayrollAppError::BootstrapOperatorAlreadyExists));

    // The refusal must be a refusal to run at all: no second Operator, and
    // no Employer, is left behind by the attempt.
    let second_attempt = find_operator_by_email(&db, "alice@example.com")
        .await
        .unwrap();
    assert_eq!(second_attempt, None);
}

#[sqlx::test]
async fn bootstrap_refuses_a_period_end_day_outside_1_to_28_and_persists_nothing(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);

    let result = bootstrap(
        &db,
        "alice@example.com",
        "Alice",
        "correct horse battery staple",
        "Acme Corp",
        BootstrapPeriodEndDay::Day(29),
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::BootstrapPeriodEndDayInvalid { day: 29 })
    );
    assert_eq!(
        find_operator_by_email(&db, "alice@example.com")
            .await
            .unwrap(),
        None
    );
}

#[sqlx::test]
async fn a_failure_partway_through_leaves_no_operator_behind(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);

    // The Operator insert happens first and would succeed on its own; the
    // blank Employer name is what refuses. Proving no Operator survives
    // proves the two inserts share one transaction rather than two that
    // merely run back to back.
    let result = bootstrap(
        &db,
        "alice@example.com",
        "Alice",
        "correct horse battery staple",
        "   ",
        BootstrapPeriodEndDay::Day(25),
    )
    .await;

    assert_eq!(result, Err(PayrollAppError::EmployerNameCannotBeEmpty));
    assert_eq!(
        find_operator_by_email(&db, "alice@example.com")
            .await
            .unwrap(),
        None
    );
}

#[sqlx::test]
async fn two_concurrent_bootstrap_calls_cannot_both_succeed(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);

    let first = bootstrap(
        &db,
        "alice@example.com",
        "Alice",
        "correct horse battery staple",
        "Acme Corp",
        BootstrapPeriodEndDay::Day(25),
    );
    let second = bootstrap(
        &db,
        "bob@example.com",
        "Bob",
        "correct horse battery staple",
        "Widgets Inc",
        BootstrapPeriodEndDay::LastDayOfMonth,
    );

    let (first, second) = tokio::join!(first, second);
    let results = [first, second];

    let successes = results.iter().filter(|r| r.is_ok()).count();
    let already_exists_refusals = results
        .iter()
        .filter(|r| matches!(r, Err(PayrollAppError::BootstrapOperatorAlreadyExists)))
        .count();
    assert_eq!(
        (successes, already_exists_refusals),
        (1, 1),
        "exactly one concurrent bootstrap call must succeed and the other must be refused as \
         BootstrapOperatorAlreadyExists, got {results:?}"
    );

    // Only the winner's own Operator, Employer and membership exist — the
    // loser left nothing behind for `find_operator_by_email` to find.
    let winner_email = if results[0].is_ok() {
        "alice@example.com"
    } else {
        "bob@example.com"
    };
    let loser_email = if results[0].is_ok() {
        "bob@example.com"
    } else {
        "alice@example.com"
    };
    assert!(
        find_operator_by_email(&db, winner_email)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        find_operator_by_email(&db, loser_email).await.unwrap(),
        None
    );
}
