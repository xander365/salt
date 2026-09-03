//! `ResolveEmployerAccess` (issue #47, parent #38 Spec 1 of 3;
//! `docs/domain/operator-auth-http-web-grill.md` §0.7-§0.10, ADR-0017). The
//! single joined query behind `salt-server`'s `AuthorizedEmployerContext`
//! extractor: `session`, `operator` and `employer_membership`, read together
//! on every call and never cached, so a revocation or a disabling bites on
//! the Operator's very next request.
//!
//! Deliberately one function, not `load_session` followed by
//! `active_membership_role`: two round trips would let a caller observe (or
//! a future change silently introduce) a gap between "the session is still
//! valid" and "the membership still is", and ADR-0017's own freshness
//! guarantee is that no such gap exists.

use chrono::{DateTime, Utc};
use payroll::EmployerId;

use crate::database::SaltDatabase;
use crate::error::PayrollAppError;
use crate::membership::{MembershipRole, membership_role_from_column};
use crate::operator::{OperatorId, OperatorStatus, operator_status_from_column};
use crate::session::{hash_token, idle_extension_threshold, idle_timeout};

/// An Employer identity that only a successful [`resolve_employer_access`]
/// can mint. It is the capability an Employer-scoped HTTP handler receives
/// from `AuthorizedEmployerContext`: unlike [`payroll::EmployerId`], its
/// constructor is not public, so a handler cannot turn a path segment into
/// one and bypass authorization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedEmployerId(EmployerId);

impl AuthorizedEmployerId {
    fn new(employer_id: EmployerId) -> Self {
        Self(employer_id)
    }

    /// The stable Employer identifier for presentation and logging. This
    /// intentionally offers no conversion back to [`payroll::EmployerId`]:
    /// future HTTP-facing use cases accept this capability directly.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for AuthorizedEmployerId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

/// What one request is entitled to do against one Employer, resolved by
/// [`resolve_employer_access`]. `salt-server`'s extractor maps this to the
/// status ADR-0017 names for each case: [`Self::Unauthenticated`] and
/// [`Self::NotAMember`] both answer without revealing which is true of a
/// given Employer id — 401 and 404 respectively — and only [`Self::Member`]
/// can ever earn a 403, when its `role` turns out too low for the route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmployerAccess {
    /// No session names an active Operator: a missing or unknown token, an
    /// expired session, or a live session naming an Operator who is
    /// [`OperatorStatus::Disabled`] — indistinguishable from outside, the
    /// same discipline `who_am_i` already applies (issue #46).
    Unauthenticated,
    /// The session is valid and the Operator is active, but no *active*
    /// `EmployerMembership` joins them to the Employer asked for. A revoked
    /// membership answers this too, never [`Self::Member`] — revoking is
    /// what this variant exists to make bite immediately.
    NotAMember,
    /// The session is valid, the Operator is active, and an active
    /// `EmployerMembership` grants `role` for the Employer asked for.
    Member {
        operator_id: OperatorId,
        employer_id: AuthorizedEmployerId,
        role: MembershipRole,
    },
}

/// Resolves `token`'s access to `employer_id` as of `now`, in the one joined
/// query issue #47's own acceptance criteria demand: `session` (does the
/// token name a live session), `operator` (is that Operator active) and
/// `employer_membership` (does that Operator hold an active membership for
/// `employer_id`) — read together, with no caching, so an Operator disabled
/// or a membership revoked mid-session is refused on their very next
/// request.
///
/// A valid session's `last_seen_at` is extended exactly as
/// [`crate::load_session`]'s own doc describes — only once it is more than
/// `idle_extension_threshold()` stale, on its own statement outside any
/// transaction the caller holds — because this function is, for every
/// Employer-scoped route, what `load_session` is for `GET /api/session`:
/// the read that proves the session is still live. Skipping the extension
/// here would mean a browser that only ever calls Employer-scoped routes
/// slowly idles out despite being used continuously.
pub async fn resolve_employer_access(
    db: &SaltDatabase,
    token: &str,
    raw_employer_id: &str,
    now: DateTime<Utc>,
) -> Result<EmployerAccess, PayrollAppError> {
    let token_hash = hash_token(token);
    let employer_id = EmployerId::new(raw_employer_id);

    type Row = (String, String, DateTime<Utc>, String, Option<String>);

    let row: Option<Row> = sqlx::query_as(
        "SELECT session.id::text, session.operator_id::text, session.last_seen_at,
                operator.status, employer_membership.role
         FROM session
         JOIN operator ON operator.id = session.operator_id
         LEFT JOIN employer_membership
           ON employer_membership.operator_id = session.operator_id
          AND employer_membership.employer_id = $2
          AND employer_membership.status = 'active'
         WHERE session.token_hash = $1
           AND $3 < session.expires_at
           AND $3 < session.last_seen_at + ($4 * INTERVAL '1 second')",
    )
    .bind(&token_hash)
    .bind(employer_id.as_str())
    .bind(now)
    .bind(idle_timeout().num_seconds())
    .fetch_optional(db.pool())
    .await?;

    let Some((session_id, operator_id, last_seen_at, operator_status, role)) = row else {
        // This is also a session lookup, so it retains `load_session`'s
        // lazy-deletion rule. The helper is a no-op for an unknown token.
        crate::session::delete_expired(db, &token_hash, now).await?;
        return Ok(EmployerAccess::Unauthenticated);
    };

    if operator_status_from_column(&operator_status) != OperatorStatus::Active {
        return Ok(EmployerAccess::Unauthenticated);
    }

    // Only an authenticated, active Operator's request counts as activity.
    // In particular, repeated requests with a disabled Operator's cookie
    // must not keep advancing a session that can no longer authenticate.
    if now > last_seen_at + idle_extension_threshold() {
        sqlx::query(
            "UPDATE session SET last_seen_at = $2
             WHERE id = $1::uuid AND last_seen_at < $2",
        )
        .bind(&session_id)
        .bind(now)
        .execute(db.pool())
        .await?;
    }

    Ok(match role {
        Some(role) => EmployerAccess::Member {
            operator_id: OperatorId::new(operator_id),
            employer_id: AuthorizedEmployerId::new(employer_id),
            role: membership_role_from_column(&role),
        },
        None => EmployerAccess::NotAMember,
    })
}

#[cfg(test)]
mod tests {
    use chrono::Timelike;
    use payroll::{DayOfMonth, PaySchedule, PeriodEndDay};
    use sqlx::PgPool;

    use super::*;
    use crate::employer::create_employer;
    use crate::membership::{create_employer_membership, revoke_employer_membership};
    use crate::operator::{create_operator, disable_operator};
    use crate::session::create_session;

    fn test_now() -> DateTime<Utc> {
        Utc::now()
            .with_nanosecond(0)
            .expect("zero nanoseconds is always valid")
    }

    async fn an_employer(db: &SaltDatabase) -> EmployerId {
        create_employer(
            db,
            "Acme Corp",
            PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap())),
            "test-setup",
        )
        .await
        .unwrap()
    }

    async fn an_operator(db: &SaltDatabase) -> OperatorId {
        create_operator(
            db,
            "alice@example.com",
            "Alice",
            "correct horse battery staple",
        )
        .await
        .unwrap()
    }

    #[sqlx::test]
    async fn an_unknown_token_is_unauthenticated(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;

        let access =
            resolve_employer_access(&db, "not-a-real-token", employer_id.as_str(), test_now())
                .await
                .unwrap();

        assert_eq!(access, EmployerAccess::Unauthenticated);
    }

    #[sqlx::test]
    async fn an_active_operator_with_no_membership_is_not_a_member(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;
        let operator_id = an_operator(&db).await;
        let session = create_session(&db, &operator_id, test_now()).await.unwrap();

        let access = resolve_employer_access(&db, &session.token, employer_id.as_str(), test_now())
            .await
            .unwrap();

        assert_eq!(access, EmployerAccess::NotAMember);
    }

    #[sqlx::test]
    async fn an_active_membership_grants_its_role(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;
        let operator_id = an_operator(&db).await;
        create_employer_membership(&db, &operator_id, &employer_id, MembershipRole::Owner)
            .await
            .unwrap();
        let session = create_session(&db, &operator_id, test_now()).await.unwrap();

        let access = resolve_employer_access(&db, &session.token, employer_id.as_str(), test_now())
            .await
            .unwrap();

        assert_eq!(
            access,
            EmployerAccess::Member {
                operator_id,
                employer_id: AuthorizedEmployerId::new(employer_id),
                role: MembershipRole::Owner,
            }
        );
    }

    #[sqlx::test]
    async fn a_revoked_membership_is_not_a_member(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;
        let operator_id = an_operator(&db).await;
        create_employer_membership(&db, &operator_id, &employer_id, MembershipRole::Owner)
            .await
            .unwrap();
        revoke_employer_membership(&db, &operator_id, &employer_id)
            .await
            .unwrap();
        let session = create_session(&db, &operator_id, test_now()).await.unwrap();

        let access = resolve_employer_access(&db, &session.token, employer_id.as_str(), test_now())
            .await
            .unwrap();

        assert_eq!(access, EmployerAccess::NotAMember);
    }

    #[sqlx::test]
    async fn a_disabled_operator_is_unauthenticated_and_does_not_extend_the_session(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool.clone());
        let employer_id = an_employer(&db).await;
        let operator_id = an_operator(&db).await;
        create_employer_membership(&db, &operator_id, &employer_id, MembershipRole::Owner)
            .await
            .unwrap();
        let created_at = test_now();
        let session = create_session(&db, &operator_id, created_at).await.unwrap();
        disable_operator(&db, &operator_id).await.unwrap();

        let access = resolve_employer_access(
            &db,
            &session.token,
            employer_id.as_str(),
            created_at + chrono::Duration::minutes(6),
        )
        .await
        .unwrap();

        assert_eq!(access, EmployerAccess::Unauthenticated);
        let last_seen_at: DateTime<Utc> =
            sqlx::query_scalar("SELECT last_seen_at FROM session WHERE id = $1::uuid")
                .bind(session.id.as_str())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(last_seen_at.timestamp(), created_at.timestamp());
    }

    #[sqlx::test]
    async fn an_expired_session_is_unauthenticated_and_deleted(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool.clone());
        let employer_id = an_employer(&db).await;
        let operator_id = an_operator(&db).await;
        create_employer_membership(&db, &operator_id, &employer_id, MembershipRole::Owner)
            .await
            .unwrap();
        let created_at = test_now();
        let session = create_session(&db, &operator_id, created_at).await.unwrap();
        let past_the_absolute_timer =
            created_at + chrono::Duration::hours(12) + chrono::Duration::seconds(1);

        let access = resolve_employer_access(
            &db,
            &session.token,
            employer_id.as_str(),
            past_the_absolute_timer,
        )
        .await
        .unwrap();

        assert_eq!(access, EmployerAccess::Unauthenticated);
        let remaining_session: Option<String> =
            sqlx::query_scalar("SELECT id::text FROM session WHERE id = $1::uuid")
                .bind(session.id.as_str())
                .fetch_optional(&pool)
                .await
                .unwrap();
        assert_eq!(remaining_session, None);
    }

    #[sqlx::test]
    async fn a_session_is_unauthenticated_exactly_on_its_idle_deadline(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;
        let operator_id = an_operator(&db).await;
        create_employer_membership(&db, &operator_id, &employer_id, MembershipRole::Owner)
            .await
            .unwrap();
        let created_at = test_now();
        let session = create_session(&db, &operator_id, created_at).await.unwrap();

        let access = resolve_employer_access(
            &db,
            &session.token,
            employer_id.as_str(),
            created_at + chrono::Duration::hours(8),
        )
        .await
        .unwrap();

        assert_eq!(access, EmployerAccess::Unauthenticated);
    }

    #[sqlx::test]
    async fn a_session_is_unauthenticated_exactly_on_its_absolute_deadline(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let employer_id = an_employer(&db).await;
        let operator_id = an_operator(&db).await;
        create_employer_membership(&db, &operator_id, &employer_id, MembershipRole::Owner)
            .await
            .unwrap();
        let created_at = test_now();
        let session = create_session(&db, &operator_id, created_at).await.unwrap();

        let six_hours_later = created_at + chrono::Duration::hours(6);
        assert!(matches!(
            resolve_employer_access(&db, &session.token, employer_id.as_str(), six_hours_later,)
                .await
                .unwrap(),
            EmployerAccess::Member { .. }
        ));

        let access = resolve_employer_access(
            &db,
            &session.token,
            employer_id.as_str(),
            created_at + chrono::Duration::hours(12),
        )
        .await
        .unwrap();

        assert_eq!(access, EmployerAccess::Unauthenticated);
    }

    #[sqlx::test]
    async fn a_membership_for_a_different_employer_is_not_a_member(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let own_employer = an_employer(&db).await;
        let other_employer = an_employer(&db).await;
        let operator_id = an_operator(&db).await;
        create_employer_membership(&db, &operator_id, &own_employer, MembershipRole::Owner)
            .await
            .unwrap();
        let session = create_session(&db, &operator_id, test_now()).await.unwrap();

        let access =
            resolve_employer_access(&db, &session.token, other_employer.as_str(), test_now())
                .await
                .unwrap();

        assert_eq!(access, EmployerAccess::NotAMember);
    }

    #[sqlx::test]
    async fn last_seen_at_is_extended_only_once_it_is_more_than_five_minutes_stale(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool.clone());
        let employer_id = an_employer(&db).await;
        let operator_id = an_operator(&db).await;
        create_employer_membership(&db, &operator_id, &employer_id, MembershipRole::Owner)
            .await
            .unwrap();
        let created_at = test_now();
        let session = create_session(&db, &operator_id, created_at).await.unwrap();

        let at_threshold = created_at + chrono::Duration::minutes(5);
        resolve_employer_access(&db, &session.token, employer_id.as_str(), at_threshold)
            .await
            .unwrap();

        let last_seen_at: DateTime<Utc> =
            sqlx::query_scalar("SELECT last_seen_at FROM session WHERE id = $1::uuid")
                .bind(session.id.as_str())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(last_seen_at.timestamp(), created_at.timestamp());

        let now_stale = created_at + chrono::Duration::minutes(6);
        resolve_employer_access(&db, &session.token, employer_id.as_str(), now_stale)
            .await
            .unwrap();

        let last_seen_at: DateTime<Utc> =
            sqlx::query_scalar("SELECT last_seen_at FROM session WHERE id = $1::uuid")
                .bind(session.id.as_str())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(last_seen_at.timestamp(), now_stale.timestamp());
    }
}
