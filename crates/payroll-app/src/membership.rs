//! `CreateEmployerMembership`, `ListEmployerMemberships`,
//! `RevokeEmployerMembership` and `ActiveMembershipRole` (issue #43, parent
//! #38). An `EmployerMembership` grants one Operator access to one Employer
//! with a role, and is itself the authorization decision (ADR-0017):
//! nothing here or later makes an `EmployerId` alone sufficient for
//! anything.
//!
//! Ships no HTTP route and no administrative API (§0.39) — through Spec 3
//! the only caller outside tests is bootstrap's first Owner membership.

use payroll::EmployerId;

use crate::database::{SaltDatabase, is_unique_violation};
use crate::error::PayrollAppError;
use crate::operator::OperatorId;

/// A membership's role (§0.5, §0.6). `Owner` covers membership
/// administration and Employer configuration; everything payroll is
/// `PayrollOperator` or above; reads are open to both. Modelled as an enum
/// now, before any route enforces the distinction, because retrofitting a
/// role column onto live memberships later is far more expensive than
/// carrying it from the start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipRole {
    Owner,
    PayrollOperator,
}

/// A membership's status. Revoked by this column, never a delete — the same
/// discipline [`crate::OperatorStatus`] applies to a disabled Operator: the
/// grant and its later revocation are both facts an audit trail should keep
/// naming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipStatus {
    Active,
    Revoked,
}

/// One EmployerMembership read back by [`list_employer_memberships`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmployerMembershipSnapshot {
    pub employer_id: EmployerId,
    pub role: MembershipRole,
    pub status: MembershipStatus,
}

/// The name PostgreSQL gives `employer_membership`'s own primary key — the
/// constraint that makes "unique on the pair" true regardless of how two
/// concurrent grants race.
const EMPLOYER_MEMBERSHIP_PKEY: &str = "employer_membership_pkey";

/// Grants `operator_id` access to `employer_id` under `role`.
///
/// Both parents are checked inside the insert itself, via `WHERE EXISTS`,
/// the same discipline [`crate::create_employment`] applies to its own
/// Employer check — so there is no window between checking and inserting.
/// A pair that already has a membership, active or revoked, is refused by
/// the table's own primary key rather than an application-side check racing
/// it; this ticket ships no path back from a revoked membership to an
/// active one.
pub async fn create_employer_membership(
    db: &SaltDatabase,
    operator_id: &OperatorId,
    employer_id: &EmployerId,
    role: MembershipRole,
) -> Result<(), PayrollAppError> {
    let mut tx = db.pool().begin().await?;
    insert_employer_membership(&mut tx, operator_id, employer_id, role).await?;
    tx.commit().await?;
    Ok(())
}

/// [`create_employer_membership`]'s own insert, on a caller-supplied
/// transaction rather than a fresh one — the same split
/// [`crate::operator::insert_operator`] makes of
/// [`crate::operator::create_operator`], and for the same reason:
/// [`crate::bootstrap::bootstrap`] needs this membership's insert in the
/// same transaction as the Operator and Employer it also writes, both of
/// which this call's own `WHERE EXISTS` then finds already present —
/// visible to it because a transaction always sees its own prior writes.
pub(crate) async fn insert_employer_membership(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operator_id: &OperatorId,
    employer_id: &EmployerId,
    role: MembershipRole,
) -> Result<(), PayrollAppError> {
    let inserted = sqlx::query(
        "INSERT INTO employer_membership (operator_id, employer_id, role)
         SELECT $1::uuid, $2, $3
         WHERE EXISTS (SELECT 1 FROM operator WHERE id = $1::uuid)
           AND EXISTS (SELECT 1 FROM employer WHERE id = $2)",
    )
    .bind(operator_id.as_str())
    .bind(employer_id.as_str())
    .bind(role_as_db_str(role))
    .execute(&mut **tx)
    .await;

    let inserted = match inserted {
        Ok(inserted) => inserted,
        Err(err) if is_unique_violation(&err, EMPLOYER_MEMBERSHIP_PKEY) => {
            return Err(PayrollAppError::EmployerMembershipAlreadyExists {
                operator_id: operator_id.clone(),
                employer_id: employer_id.clone(),
            });
        }
        Err(err) => return Err(err.into()),
    };

    if inserted.rows_affected() == 0 {
        return Err(diagnose_missing_parent(tx, operator_id, employer_id).await?);
    }

    Ok(())
}

/// Tells an unknown Operator apart from an unknown Employer once
/// [`insert_employer_membership`]'s combined `WHERE EXISTS` has already
/// found one of them missing. A second pair of lookups rather than folding
/// this into the insert itself: the insert must stay a single statement for
/// the no-window property above, and this path is only ever reached once
/// that statement has already refused.
async fn diagnose_missing_parent(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operator_id: &OperatorId,
    employer_id: &EmployerId,
) -> Result<PayrollAppError, PayrollAppError> {
    let operator_exists: Option<bool> =
        sqlx::query_scalar("SELECT TRUE FROM operator WHERE id = $1::uuid")
            .bind(operator_id.as_str())
            .fetch_optional(&mut **tx)
            .await?;

    Ok(if operator_exists.is_none() {
        PayrollAppError::OperatorNotFound(operator_id.clone())
    } else {
        PayrollAppError::EmployerNotFound(employer_id.clone())
    })
}

/// Every EmployerMembership `operator_id` holds, active or revoked, in
/// creation order — what lets an Operator belong to several Employers under
/// a single identity and have all of them found. Only that Operator's own
/// memberships: the `WHERE` below is the explicit filter ADR-0017 names as
/// the second layer, in place of a database policy.
///
/// `employer_id` breaks a tie on `created_at`, which is `now()` and so is
/// the *transaction's* timestamp: two memberships granted in one
/// transaction carry the identical instant, and without the tiebreaker
/// PostgreSQL would be free to return them in either order on either call.
/// Creation order is not observable inside one transaction anyway, so the
/// tiebreaker costs nothing and makes the order total and stable.
pub async fn list_employer_memberships(
    db: &SaltDatabase,
    operator_id: &OperatorId,
) -> Result<Vec<EmployerMembershipSnapshot>, PayrollAppError> {
    type Row = (String, String, String);

    let rows: Vec<Row> = sqlx::query_as(
        "SELECT employer_id, role, status FROM employer_membership
         WHERE operator_id = $1::uuid
         ORDER BY created_at, employer_id",
    )
    .bind(operator_id.as_str())
    .fetch_all(db.pool())
    .await?;

    Ok(rows
        .into_iter()
        .map(|(employer_id, role, status)| EmployerMembershipSnapshot {
            employer_id: EmployerId::new(employer_id),
            role: membership_role_from_column(&role),
            status: membership_status_from_column(&status),
        })
        .collect())
}

/// The role `operator_id` holds for `employer_id`, or `None` when no
/// membership exists or the one that does is `revoked`.
///
/// This is the read a later spec's `AuthorizedEmployerContext` joins into a
/// session and Operator status (ADR-0017, §0.7, §0.8): one indexed query,
/// read fresh on every call and never cached, so a revocation grants nothing
/// from its very next read with no invalidation mechanism to design.
///
/// It answers one question only — *does this membership grant this role
/// right now* — and deliberately says nothing about the Operator. A
/// disabled Operator still holds their memberships, and this function still
/// reports them, because disabling is a fact about the Operator and not
/// about the grant. Operator status is the **separate** condition ADR-0017
/// puts in the same joined query, so a caller authorizing a request must
/// check it too; `Some(role)` here is one of three answers that must all
/// hold, never authorization on its own.
pub async fn active_membership_role(
    db: &SaltDatabase,
    operator_id: &OperatorId,
    employer_id: &EmployerId,
) -> Result<Option<MembershipRole>, PayrollAppError> {
    let role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM employer_membership
         WHERE operator_id = $1::uuid AND employer_id = $2 AND status = 'active'",
    )
    .bind(operator_id.as_str())
    .bind(employer_id.as_str())
    .fetch_optional(db.pool())
    .await?;

    Ok(role.map(|role| membership_role_from_column(&role)))
}

/// Revokes the membership `operator_id` holds for `employer_id`. Revoking an
/// already-revoked membership is refused rather than repeated, the same
/// discipline [`crate::disable_operator`] applies to a second disabling: the
/// revocation has already happened, and a second one would record an act
/// that did not.
pub async fn revoke_employer_membership(
    db: &SaltDatabase,
    operator_id: &OperatorId,
    employer_id: &EmployerId,
) -> Result<(), PayrollAppError> {
    let revoked: Option<bool> = sqlx::query_scalar(
        "UPDATE employer_membership SET status = 'revoked'
         WHERE operator_id = $1::uuid AND employer_id = $2 AND status = 'active'
         RETURNING TRUE",
    )
    .bind(operator_id.as_str())
    .bind(employer_id.as_str())
    .fetch_optional(db.pool())
    .await?;

    if revoked.is_some() {
        return Ok(());
    }

    let exists: Option<bool> = sqlx::query_scalar(
        "SELECT TRUE FROM employer_membership
         WHERE operator_id = $1::uuid AND employer_id = $2",
    )
    .bind(operator_id.as_str())
    .bind(employer_id.as_str())
    .fetch_optional(db.pool())
    .await?;

    Err(match exists {
        Some(_) => PayrollAppError::EmployerMembershipAlreadyRevoked {
            operator_id: operator_id.clone(),
            employer_id: employer_id.clone(),
        },
        None => PayrollAppError::EmployerMembershipNotFound {
            operator_id: operator_id.clone(),
            employer_id: employer_id.clone(),
        },
    })
}

fn role_as_db_str(role: MembershipRole) -> &'static str {
    match role {
        MembershipRole::Owner => "owner",
        MembershipRole::PayrollOperator => "payroll_operator",
    }
}

/// The inverse of [`role_as_db_str`]. Panics rather than returning a
/// `Result`: the table's own CHECK constraint already guarantees `role` is
/// one of these two strings, so disagreement here means the schema no
/// longer matches this code, not a fact about the membership being read.
pub(crate) fn membership_role_from_column(role: &str) -> MembershipRole {
    match role {
        "owner" => MembershipRole::Owner,
        "payroll_operator" => MembershipRole::PayrollOperator,
        other => panic!(
            "employer_membership CHECK: role is 'owner' or 'payroll_operator', found {other:?}"
        ),
    }
}

/// The inverse of `status`'s column CHECK, panicking on the same discipline
/// as [`membership_role_from_column`].
fn membership_status_from_column(status: &str) -> MembershipStatus {
    match status {
        "active" => MembershipStatus::Active,
        "revoked" => MembershipStatus::Revoked,
        other => {
            panic!("employer_membership CHECK: status is 'active' or 'revoked', found {other:?}")
        }
    }
}

#[cfg(test)]
mod tests {
    use payroll::{DayOfMonth, PaySchedule, PeriodEndDay};
    use sqlx::PgPool;

    use super::*;
    use crate::employer::create_employer;

    /// `OperatorId::new` is `pub(crate)`, so only a test inside this crate
    /// can hand `create_employer_membership` an id shaped like a real
    /// Operator's that names no row — the case an external caller can never
    /// construct, but a stale session or a bad request body still can.
    #[sqlx::test]
    async fn creating_a_membership_for_an_unknown_operator_is_refused(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = create_employer(
            &db,
            "Acme Corp",
            PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap())),
            "actor",
        )
        .await
        .unwrap();
        let unknown_operator = OperatorId::new(crate::ids::new_id());

        let result =
            create_employer_membership(&db, &unknown_operator, &employer_id, MembershipRole::Owner)
                .await;

        assert_eq!(
            result,
            Err(PayrollAppError::OperatorNotFound(unknown_operator))
        );
    }
}
