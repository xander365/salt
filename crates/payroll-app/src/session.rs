//! `CreateSession`, `LoadSession` and `DeleteSession` — how a signed-in
//! Operator stays signed in (issue #44, parent #38). A session is authorized
//! by nothing but its own row: an idle timer and an absolute timer are both
//! enforced by the query that loads it, so a caller reading `Some` already
//! holds a live grant and never a fact it must separately check the clock
//! against.
//!
//! Authenticating an Operator against a password is `operator.rs`'s table
//! (issue #41); authorizing one against an Employer is `membership.rs`'s
//! (issue #43). This module only carries the identity forward between
//! requests.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Duration, Utc};
use rand::RngCore;
use rand::rngs::OsRng;
use sha2::{Digest, Sha256};

use crate::database::SaltDatabase;
use crate::error::PayrollAppError;
use crate::ids::{app_id, new_id};
use crate::operator::OperatorId;

app_id! {
    /// `payroll-app`'s own id for a Session (§4.1-style: a native `uuid`
    /// column), minted the same way [`OperatorId`] is. Never itself the
    /// bearer credential — a caller authenticates with the plaintext token
    /// [`create_session`] hands back, not with this id.
    SessionId
}

/// The token's width in bytes (256 bits), the minimum this ticket's own
/// acceptance criteria names.
const TOKEN_BYTES: usize = 32;

/// The absolute timer: a session stops loading `created_at + this` after it
/// was created, no matter how recently it was used. Twelve hours, this
/// module's own instruction — stated once here, in Rust, so
/// [`create_session`] (which writes `session.expires_at` from it) and
/// [`load_session`] (which reads that column back) cannot drift apart the
/// way restating "12 hours" a second time as a SQL literal would risk.
fn absolute_timeout() -> Duration {
    Duration::hours(12)
}

/// The idle timer: a session stops loading `last_seen_at + this` after it
/// was last seen. Eight hours, checked live by [`load_session`] rather than
/// stored, which is what lets `last_seen_at` advance without ever touching
/// the absolute deadline in `expires_at`.
fn idle_timeout() -> Duration {
    Duration::hours(8)
}

/// How stale `last_seen_at` must be before [`load_session`] bothers to
/// extend it. Five minutes of drift against an eight-hour idle window is not
/// a security property — it exists purely so a busy Operator's every request
/// does not each write this row.
fn idle_extension_threshold() -> Duration {
    Duration::minutes(5)
}

/// What [`create_session`] hands back: the new session's id, and — the
/// only place this ever appears — its plaintext bearer token. The database
/// holds only [`hash_token`]'s digest of it; nothing here or later stores,
/// logs, or `Debug`-prints the token itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedSession {
    pub id: SessionId,
    pub token: String,
}

/// A live session read back by [`load_session`]: the two facts an
/// authenticated request needs, and nothing about its timers — a caller
/// holding one already knows both timers passed, because the query that
/// produced it is the query that checked them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub id: SessionId,
    pub operator_id: OperatorId,
}

/// Opens a new session for `operator_id`, valid from `now` (the caller's own
/// clock reading, the same discipline [`crate::verify_operator_credential`]
/// follows, so a test can drive the timers without sleeping on the wall
/// clock).
///
/// The token is minted here, from the operating system's cryptographic
/// random source ([`OsRng`]), and only [`hash_token`]'s SHA-256 digest of it
/// is written to `session.token_hash`. The plaintext lives in the returned
/// [`CreatedSession`] and nowhere else Salt keeps.
///
/// The Operator's existence is checked inside the insert itself, via `WHERE
/// EXISTS`, the same discipline [`crate::create_employer_membership`]
/// applies to its own parent check — so there is no window between checking
/// and inserting.
pub async fn create_session(
    db: &SaltDatabase,
    operator_id: &OperatorId,
    now: DateTime<Utc>,
) -> Result<CreatedSession, PayrollAppError> {
    let token = generate_token();
    let token_hash = hash_token(&token);
    let id = SessionId::new(new_id());
    let expires_at = now + absolute_timeout();

    let inserted = sqlx::query(
        "INSERT INTO session (id, operator_id, token_hash, created_at, last_seen_at, expires_at)
         SELECT $1::uuid, $2::uuid, $3, $4, $4, $5
         WHERE EXISTS (SELECT 1 FROM operator WHERE id = $2::uuid)",
    )
    .bind(id.as_str())
    .bind(operator_id.as_str())
    .bind(&token_hash)
    .bind(now)
    .bind(expires_at)
    .execute(db.pool())
    .await?;

    if inserted.rows_affected() == 0 {
        return Err(PayrollAppError::OperatorNotFound(operator_id.clone()));
    }

    Ok(CreatedSession { id, token })
}

/// Looks `token` up and returns the [`SessionSnapshot`] it names, or `None`
/// when no row's token hashes to it, or the row that does has outlived
/// either timer. `now` is the caller's own clock reading.
///
/// Both timers are checked inside the one `SELECT` that decides validity —
/// `expires_at` (the absolute deadline [`create_session`] fixed) and
/// `last_seen_at` + [`idle_timeout`] (checked live, never stored) — so an
/// expired row is never honoured even before anything deletes it. When that
/// `SELECT` finds nothing, [`delete_expired`] re-checks the same two
/// conditions and removes the row if it is the reason: lazy deletion by the
/// lookup that found it expired, with no sweeper and no cron (this module's
/// own instruction).
///
/// A valid row's `last_seen_at` is advanced to `now` only once it is more
/// than [`idle_extension_threshold`] stale, and that write reaches the
/// database on its own statement, outside whatever transaction the caller
/// may itself hold open — a `Debug` build for the caller cannot roll it
/// back, and eight hours of idle budget can absorb five minutes of drift
/// without that write becoming a security property.
pub async fn load_session(
    db: &SaltDatabase,
    token: &str,
    now: DateTime<Utc>,
) -> Result<Option<SessionSnapshot>, PayrollAppError> {
    let token_hash = hash_token(token);

    type Row = (String, String, DateTime<Utc>);

    let valid: Option<Row> = sqlx::query_as(
        "SELECT id::text, operator_id::text, last_seen_at FROM session
         WHERE token_hash = $1
           AND $2 < expires_at
           AND $2 < last_seen_at + ($3 * INTERVAL '1 second')",
    )
    .bind(&token_hash)
    .bind(now)
    .bind(idle_timeout().num_seconds())
    .fetch_optional(db.pool())
    .await?;

    let Some((id, operator_id, last_seen_at)) = valid else {
        delete_expired(db, &token_hash, now).await?;
        return Ok(None);
    };

    if now > last_seen_at + idle_extension_threshold() {
        sqlx::query("UPDATE session SET last_seen_at = $2 WHERE id = $1::uuid")
            .bind(&id)
            .bind(now)
            .execute(db.pool())
            .await?;
    }

    Ok(Some(SessionSnapshot {
        id: SessionId::new(id),
        operator_id: OperatorId::new(operator_id),
    }))
}

/// Deletes the row `token_hash` names if, as of `now`, it has outlived
/// either timer — the same two conditions [`load_session`]'s own `SELECT`
/// checks, recomputed rather than reused so this is safe to call whenever
/// that `SELECT` found nothing, including when `token_hash` names no row at
/// all (the `DELETE` then matches nothing and is a no-op).
async fn delete_expired(
    db: &SaltDatabase,
    token_hash: &str,
    now: DateTime<Utc>,
) -> Result<(), PayrollAppError> {
    sqlx::query(
        "DELETE FROM session
         WHERE token_hash = $1
           AND NOT ($2 < expires_at AND $2 < last_seen_at + ($3 * INTERVAL '1 second'))",
    )
    .bind(token_hash)
    .bind(now)
    .bind(idle_timeout().num_seconds())
    .execute(db.pool())
    .await?;

    Ok(())
}

/// Deletes the session named by `session_id` — an Operator signing out.
/// Idempotent rather than refusing an unknown or already-gone id: signing
/// out of a session that is already gone reaches the same end state either
/// way, and unlike disabling an Operator or revoking a membership there is
/// no audit trail here that a second delete would falsely claim happened
/// twice.
pub async fn delete_session(
    db: &SaltDatabase,
    session_id: &SessionId,
) -> Result<(), PayrollAppError> {
    sqlx::query("DELETE FROM session WHERE id = $1::uuid")
        .bind(session_id.as_str())
        .execute(db.pool())
        .await?;

    Ok(())
}

/// A fresh bearer token: [`TOKEN_BYTES`] (256 bits) from [`OsRng`], the
/// operating system's cryptographic random source, encoded URL-safe base64
/// with no padding so it drops into a URL, a header, or a cookie unescaped.
fn generate_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The digest [`create_session`] writes and [`load_session`] looks up by.
/// Plain SHA-256, not a password KDF: the token is already high-entropy
/// (§ this module's own doc), so a slow hash would cost a hash per request
/// and buy nothing (ADR-0016).
fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;
    use crate::database::SaltDatabase;

    /// `OperatorId::new` is `pub(crate)`, so only a test inside this crate
    /// can hand `create_session` an id shaped like a real Operator's that
    /// names no row — the same note `membership.rs`'s own test makes of its
    /// own parent check.
    #[sqlx::test]
    async fn creating_a_session_for_an_unknown_operator_is_refused(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let unknown_operator = OperatorId::new(new_id());

        let result = create_session(&db, &unknown_operator, Utc::now()).await;

        assert_eq!(
            result,
            Err(PayrollAppError::OperatorNotFound(unknown_operator))
        );
    }

    #[test]
    fn hashing_is_deterministic_and_distinguishes_tokens() {
        assert_eq!(hash_token("a-token"), hash_token("a-token"));
        assert_ne!(hash_token("a-token"), hash_token("a-different-token"));
    }

    #[test]
    fn a_hashed_token_is_a_sha256_hex_digest() {
        let digest = hash_token("a-token");

        assert_eq!(
            digest.len(),
            64,
            "a SHA-256 digest is 32 bytes, 64 hex chars"
        );
        assert!(digest.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn generated_tokens_carry_at_least_256_bits_and_are_not_repeated() {
        let first = generate_token();
        let second = generate_token();

        assert_ne!(first, second);
        // URL_SAFE_NO_PAD encodes 32 bytes as 43 base64 characters (ceil(32*8/6),
        // no padding) — proving the length proves the entropy width without
        // decoding back to bytes.
        assert_eq!(first.chars().count(), 43);
        assert!(
            first
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }
}
