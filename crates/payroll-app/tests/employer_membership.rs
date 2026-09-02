//! Proves the use cases issue #43 introduces: `create_employer_membership`,
//! `list_employer_memberships`, `active_membership_role` and
//! `revoke_employer_membership` — reached through the public API a later
//! ticket's HTTP layer calls, not raw SQL.

use payroll::{DayOfMonth, PeriodEndDay};
use payroll_app::{
    EmployerMembershipSnapshot, MembershipRole, MembershipStatus, PayrollAppError, SaltDatabase,
    active_membership_role, create_employer, create_employer_membership, create_operator,
    list_employer_memberships, revoke_employer_membership,
};
use sqlx::PgPool;

fn schedule() -> payroll::PaySchedule {
    payroll::PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap()))
}

#[sqlx::test]
async fn a_membership_is_created_and_then_found_when_listing_that_operators_memberships(
    pool: PgPool,
) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = create_operator(&db, "alice@example.com", "Alice", "a password")
        .await
        .unwrap();
    let employer_id = create_employer(&db, "Acme Corp", schedule(), "actor")
        .await
        .unwrap();

    create_employer_membership(&db, &operator_id, &employer_id, MembershipRole::Owner)
        .await
        .unwrap();

    let memberships = list_employer_memberships(&db, &operator_id).await.unwrap();

    assert_eq!(
        memberships,
        vec![EmployerMembershipSnapshot {
            employer_id: employer_id.clone(),
            role: MembershipRole::Owner,
            status: MembershipStatus::Active,
        }]
    );
}

#[sqlx::test]
async fn an_operator_may_hold_memberships_for_several_employers_and_all_are_returned(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = create_operator(&db, "alice@example.com", "Alice", "a password")
        .await
        .unwrap();
    let first_employer = create_employer(&db, "First Corp", schedule(), "actor")
        .await
        .unwrap();
    let second_employer = create_employer(&db, "Second Corp", schedule(), "actor")
        .await
        .unwrap();

    create_employer_membership(&db, &operator_id, &first_employer, MembershipRole::Owner)
        .await
        .unwrap();
    create_employer_membership(
        &db,
        &operator_id,
        &second_employer,
        MembershipRole::PayrollOperator,
    )
    .await
    .unwrap();

    let memberships = list_employer_memberships(&db, &operator_id).await.unwrap();

    assert_eq!(memberships.len(), 2);
    assert!(
        memberships
            .iter()
            .any(|m| m.employer_id == first_employer && m.role == MembershipRole::Owner)
    );
    assert!(
        memberships
            .iter()
            .any(|m| m.employer_id == second_employer && m.role == MembershipRole::PayrollOperator)
    );
}

/// Both roles are modelled and distinguishable (issue #43's own acceptance
/// criterion): the value round-trips through the database rather than being
/// collapsed to one thing on the way back out.
#[sqlx::test]
async fn both_roles_are_modelled_and_distinguishable(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let owner = create_operator(&db, "owner@example.com", "Owner", "a password")
        .await
        .unwrap();
    let payroll_operator = create_operator(&db, "operator@example.com", "Operator", "a password")
        .await
        .unwrap();
    let employer_id = create_employer(&db, "Acme Corp", schedule(), "actor")
        .await
        .unwrap();

    create_employer_membership(&db, &owner, &employer_id, MembershipRole::Owner)
        .await
        .unwrap();
    create_employer_membership(
        &db,
        &payroll_operator,
        &employer_id,
        MembershipRole::PayrollOperator,
    )
    .await
    .unwrap();

    assert_eq!(
        active_membership_role(&db, &owner, &employer_id)
            .await
            .unwrap(),
        Some(MembershipRole::Owner)
    );
    assert_eq!(
        active_membership_role(&db, &payroll_operator, &employer_id)
            .await
            .unwrap(),
        Some(MembershipRole::PayrollOperator)
    );
}

#[sqlx::test]
async fn creating_a_membership_for_an_unknown_employer_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = create_operator(&db, "alice@example.com", "Alice", "a password")
        .await
        .unwrap();
    let unknown_employer = payroll::EmployerId::new("no-such-employer");

    let result =
        create_employer_membership(&db, &operator_id, &unknown_employer, MembershipRole::Owner)
            .await;

    assert_eq!(
        result,
        Err(PayrollAppError::EmployerNotFound(unknown_employer))
    );
}

#[sqlx::test]
async fn creating_a_second_membership_for_the_same_pair_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = create_operator(&db, "alice@example.com", "Alice", "a password")
        .await
        .unwrap();
    let employer_id = create_employer(&db, "Acme Corp", schedule(), "actor")
        .await
        .unwrap();

    create_employer_membership(&db, &operator_id, &employer_id, MembershipRole::Owner)
        .await
        .unwrap();

    let result = create_employer_membership(
        &db,
        &operator_id,
        &employer_id,
        MembershipRole::PayrollOperator,
    )
    .await;

    assert_eq!(
        result,
        Err(PayrollAppError::EmployerMembershipAlreadyExists {
            operator_id,
            employer_id,
        })
    );
}

/// The whole point of the ticket: a revoked membership no longer grants
/// access.
#[sqlx::test]
async fn a_revoked_membership_no_longer_grants_access(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = create_operator(&db, "alice@example.com", "Alice", "a password")
        .await
        .unwrap();
    let employer_id = create_employer(&db, "Acme Corp", schedule(), "actor")
        .await
        .unwrap();
    create_employer_membership(&db, &operator_id, &employer_id, MembershipRole::Owner)
        .await
        .unwrap();

    assert_eq!(
        active_membership_role(&db, &operator_id, &employer_id)
            .await
            .unwrap(),
        Some(MembershipRole::Owner)
    );

    revoke_employer_membership(&db, &operator_id, &employer_id)
        .await
        .unwrap();

    assert_eq!(
        active_membership_role(&db, &operator_id, &employer_id)
            .await
            .unwrap(),
        None,
        "a revoked membership must grant nothing"
    );

    let memberships = list_employer_memberships(&db, &operator_id).await.unwrap();
    assert_eq!(
        memberships,
        vec![EmployerMembershipSnapshot {
            employer_id,
            role: MembershipRole::Owner,
            status: MembershipStatus::Revoked,
        }],
        "revocation is a status change, not a delete — the row stays for the audit trail"
    );
}

#[sqlx::test]
async fn revoking_an_unknown_membership_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = create_operator(&db, "alice@example.com", "Alice", "a password")
        .await
        .unwrap();
    let employer_id = create_employer(&db, "Acme Corp", schedule(), "actor")
        .await
        .unwrap();

    let result = revoke_employer_membership(&db, &operator_id, &employer_id).await;

    assert_eq!(
        result,
        Err(PayrollAppError::EmployerMembershipNotFound {
            operator_id,
            employer_id,
        })
    );
}

#[sqlx::test]
async fn revoking_an_already_revoked_membership_is_refused(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = create_operator(&db, "alice@example.com", "Alice", "a password")
        .await
        .unwrap();
    let employer_id = create_employer(&db, "Acme Corp", schedule(), "actor")
        .await
        .unwrap();
    create_employer_membership(&db, &operator_id, &employer_id, MembershipRole::Owner)
        .await
        .unwrap();
    revoke_employer_membership(&db, &operator_id, &employer_id)
        .await
        .unwrap();

    let result = revoke_employer_membership(&db, &operator_id, &employer_id).await;

    assert_eq!(
        result,
        Err(PayrollAppError::EmployerMembershipAlreadyRevoked {
            operator_id,
            employer_id,
        })
    );
}

#[sqlx::test]
async fn an_operator_with_no_memberships_lists_none(pool: PgPool) {
    let db = SaltDatabase::from_pool(pool);
    let operator_id = create_operator(&db, "alice@example.com", "Alice", "a password")
        .await
        .unwrap();

    let memberships = list_employer_memberships(&db, &operator_id).await.unwrap();

    assert_eq!(memberships, vec![]);
}
