//! `CreateOperator`, `FindOperatorByEmail`, `DisableOperator` and
//! `VerifyOperatorCredential` — the global human identity Salt can name
//! (issue #41, parent #38;
//! `docs/domain/operator-auth-http-web-grill.md` §6, §9, §0.12a-§0.13).
//! `VerifyOperatorCredential` also carries issue #42's lockout: ten failures
//! inside a fifteen-minute window lock the account for fifteen minutes from
//! the failure that locked it, self-lifting rather than administrator-cleared.
//!
//! An Operator is authenticated here; authorizing one against an Employer
//! through `EmployerMembership` is a later spec's table, not this one's.

use argon2::password_hash::phc::PasswordHash;
use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use chrono::{DateTime, Duration, Utc};

use crate::database::{SaltDatabase, is_unique_violation};
use crate::error::PayrollAppError;
use crate::ids::{app_id, new_id};

app_id! {
    /// `payroll-app`'s own id for an Operator (§4.1-style: a native `uuid`
    /// column). Minted here, in Rust, as a UUIDv7 by `new_id` rather than
    /// left to a database default — issue #41's own instruction — so it
    /// sorts by creation time the same way every other id this crate mints
    /// does.
    OperatorId
}

/// An Operator's status (`docs/domain/operator-auth-http-web-grill.md` §6).
/// Typed rather than a bare string, for the same reason [`crate::ActionType`]
/// is: a caller matches on a variant instead of comparing text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperatorStatus {
    Active,
    Disabled,
}

/// An Operator read back, without its `password_verifier` — nothing in this
/// crate ever hands that column to a caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperatorSnapshot {
    pub id: OperatorId,
    pub email: String,
    pub display_name: String,
    pub status: OperatorStatus,
}

/// A fixed, valid Argon2id PHC string, generated once offline against
/// [`hasher`]'s parameters and a fixed salt — never regenerated at runtime.
/// It verifies no real Operator's password; its only purpose is to give an
/// unknown email in [`verify_operator_credential`] the same Argon2id work a
/// real row costs (§0.12a: "An unknown email does the same Argon2id work
/// against a fixed dummy verifier, so the response time does not separate
/// 'no such account' from 'wrong password'").
///
/// Being a constant, it cannot follow a later change to [`hasher`]'s
/// parameters, and a dummy cheaper than a real row would turn the timing
/// property into a timing oracle. `the_dummy_verifier_costs_what_a_real_row_costs`
/// is what makes that drift a failing build rather than a silent regression.
const DUMMY_VERIFIER: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdGR1bW15dmVyaWZpZXJzYWx0Zm9yNDF4eA$\
     6gfr+ZD2aJP77k3KUWJnrPmvgefDG4meLGL0DMD7Oy8";

/// Password limits required by the credential-handling design. Length is
/// measured in Unicode scalar values, so a password's limit is not changed
/// merely because it contains non-ASCII characters. The maximum also bounds
/// what [`verify_operator_credential`] will hash, so an unauthenticated
/// caller cannot choose how much Argon2id work one attempt costs.
const MIN_PASSWORD_LENGTH: usize = 8;
const MAX_PASSWORD_LENGTH: usize = 1024;

/// Issue #42's lockout threshold: ten failures is "far above human
/// mistyping and far below useful guessing" (the issue's own words), and is
/// deliberately not configurable in this spec.
const LOCKOUT_THRESHOLD: i32 = 10;

/// Both the width of the window failures are counted in and how long a lock
/// lasts (the issue's own words: "Ten failed attempts within fifteen minutes
/// locks the account for fifteen minutes"). Those are two separate fifteen
/// minutes, not one: the window is measured from the first failure, and the
/// lock from the tenth. `next_attempt_counters` restarts the window at the
/// failure that locks, which is what makes the second of them a full fifteen
/// minutes however far apart the ten failures were spread.
///
/// One constant serves both so they cannot drift apart, and it is the *only*
/// statement of the duration: the counter transition is computed here in
/// Rust and written as two bound values, rather than as an `INTERVAL` literal
/// inside the UPDATE that would restate the same fifteen minutes in a second
/// place.
fn lockout_window() -> Duration {
    Duration::minutes(15)
}

/// How long [`verify_operator_credential`] waits for another attempt on the
/// *same* Operator to release the row lock before refusing.
///
/// The row lock is what makes deciding the lock and moving the counter one
/// operation, and it is deliberately held across Argon2id, so concurrent
/// attempts on one account queue. Unbounded, that queue is a denial of
/// service against the whole application rather than against one account:
/// every waiter holds a connection out of a pool every other Employer's
/// requests draw on. Bounding the wait turns "the pool is gone" into "this
/// attempt is refused". Generous next to the tens of milliseconds one
/// Argon2id verification costs, so an Operator with two open tabs never
/// reaches it, and a caller cannot tell the refusal from a wrong password.
const LOCK_WAIT_TIMEOUT: Duration = Duration::seconds(5);

/// PostgreSQL's SQLSTATE for `lock_timeout` elapsing — the one database
/// error [`verify_operator_credential`] answers as a refusal rather than
/// propagating.
const LOCK_NOT_AVAILABLE: &str = "55P03";

/// The name PostgreSQL gives migration 0028's `UNIQUE INDEX
/// operator_email_folded_key ON operator (lower(email))` — the index that
/// makes "unique case-insensitively" true regardless of how two concurrent
/// creations race.
const OPERATOR_EMAIL_FOLDED_KEY: &str = "operator_email_folded_key";

/// Records a new Operator with the given `email`, `display_name` and
/// `password` (§0.13: Argon2id via the `argon2` crate). The password itself
/// is never stored — only `hash_password`'s PHC string is, and that string
/// carries its own Argon2 parameters inline, so a later change to this
/// module's default parameters still verifies rows hashed under the old
/// ones.
///
/// `email` is stored with its own capitalisation intact and only its
/// surrounding whitespace removed; a second Operator whose email folds to
/// the same lower-cased form is refused by the database's own unique index,
/// not by an application-side check racing it (see
/// `OPERATOR_EMAIL_FOLDED_KEY`).
pub async fn create_operator(
    db: &SaltDatabase,
    email: &str,
    display_name: &str,
    password: &str,
) -> Result<OperatorId, PayrollAppError> {
    let mut tx = db.pool().begin().await?;
    let id = insert_operator(&mut tx, email, display_name, password).await?;
    tx.commit().await?;
    Ok(id)
}

/// [`create_operator`]'s own insert, on a caller-supplied transaction rather
/// than a fresh one. [`crate::bootstrap::bootstrap`] is the one other
/// caller: it needs this Operator's insert in the *same* transaction as the
/// Employer and Owner membership it also writes, so "the three inserts
/// share one transaction" (issue #48) is true rather than merely close to
/// true. `create_operator` above is this function wrapped in its own
/// begin/commit, so the two can never validate or insert differently.
pub(crate) async fn insert_operator(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    email: &str,
    display_name: &str,
    password: &str,
) -> Result<OperatorId, PayrollAppError> {
    // Trimmed, not merely checked for blankness: an email is unique only
    // case-insensitively, and folding case does not make ' alice@x' collide
    // with 'alice@x'. Storing the surrounding whitespace would let two
    // accounts exist that every human reads as one. The capitalisation issue
    // #41 asks to preserve is inside the address, not around it.
    let email = email.trim();
    let display_name = display_name.trim();

    if email.is_empty() {
        return Err(PayrollAppError::OperatorEmailCannotBeEmpty);
    }
    if display_name.is_empty() {
        return Err(PayrollAppError::OperatorDisplayNameCannotBeEmpty);
    }
    let password_length = password.chars().count();
    if password_length < MIN_PASSWORD_LENGTH {
        return Err(PayrollAppError::OperatorPasswordTooShort {
            minimum: MIN_PASSWORD_LENGTH,
        });
    }
    if password_length > MAX_PASSWORD_LENGTH {
        return Err(PayrollAppError::OperatorPasswordTooLong {
            maximum: MAX_PASSWORD_LENGTH,
        });
    }

    let password_verifier = hash_password(password)?;
    let id = OperatorId::new(new_id());

    let inserted = sqlx::query(
        "INSERT INTO operator (id, email, display_name, password_verifier)
         VALUES ($1::uuid, $2, $3, $4)",
    )
    .bind(id.as_str())
    .bind(email)
    .bind(display_name)
    .bind(&password_verifier)
    .execute(&mut **tx)
    .await;

    if let Err(err) = inserted {
        if is_unique_violation(&err, OPERATOR_EMAIL_FOLDED_KEY) {
            return Err(PayrollAppError::OperatorEmailAlreadyInUse);
        }
        return Err(err.into());
    }

    Ok(id)
}

/// Reads the Operator whose email folds to `email`, or `None` when no such
/// row exists. Unlike [`verify_operator_credential`] this is not a security
/// boundary in itself — it names no credential and does the same work
/// whether or not a row exists — so it is a plain lookup, not one guarded
/// against timing.
pub async fn find_operator_by_email(
    db: &SaltDatabase,
    email: &str,
) -> Result<Option<OperatorSnapshot>, PayrollAppError> {
    type Row = (String, String, String, String);

    let row: Option<Row> = sqlx::query_as(
        "SELECT id::text, email, display_name, status FROM operator
         WHERE lower(email) = lower($1)",
    )
    .bind(email.trim())
    .fetch_optional(db.pool())
    .await?;

    Ok(
        row.map(|(id, email, display_name, status)| OperatorSnapshot {
            id: OperatorId::new(id),
            email,
            display_name,
            status: operator_status_from_column(&status),
        }),
    )
}

/// Reads the Operator named by `operator_id`, or `None` when no such row
/// exists. The counterpart [`find_operator_by_email`] uses to look an
/// Operator up by login identifier; this is what a caller who already holds
/// an [`OperatorId`] — a [`crate::SessionSnapshot`], most often — uses
/// instead. Not a security boundary any more than `find_operator_by_email`
/// is: it names no credential, so there is no timing property to protect.
pub async fn find_operator_by_id(
    db: &SaltDatabase,
    operator_id: &OperatorId,
) -> Result<Option<OperatorSnapshot>, PayrollAppError> {
    type Row = (String, String, String, String);

    let row: Option<Row> = sqlx::query_as(
        "SELECT id::text, email, display_name, status FROM operator WHERE id = $1::uuid",
    )
    .bind(operator_id.as_str())
    .fetch_optional(db.pool())
    .await?;

    Ok(
        row.map(|(id, email, display_name, status)| OperatorSnapshot {
            id: OperatorId::new(id),
            email,
            display_name,
            status: operator_status_from_column(&status),
        }),
    )
}

/// Marks an Operator `disabled` (§6: "How is an Operator disabled without
/// deleting history?" — by status, never a delete). Disabling an
/// already-disabled Operator is refused rather than repeated, the same
/// discipline [`crate::void_employment`] applies to a void: the act has
/// already happened, and a second one would record something that did not.
pub async fn disable_operator(
    db: &SaltDatabase,
    operator_id: &OperatorId,
) -> Result<(), PayrollAppError> {
    let disabled: Option<bool> = sqlx::query_scalar(
        "UPDATE operator SET status = 'disabled'
         WHERE id = $1::uuid AND status = 'active'
         RETURNING TRUE",
    )
    .bind(operator_id.as_str())
    .fetch_optional(db.pool())
    .await?;

    if disabled.is_some() {
        return Ok(());
    }

    let exists: Option<bool> = sqlx::query_scalar("SELECT TRUE FROM operator WHERE id = $1::uuid")
        .bind(operator_id.as_str())
        .fetch_optional(db.pool())
        .await?;

    Err(match exists {
        Some(_) => PayrollAppError::OperatorAlreadyDisabled(operator_id.clone()),
        None => PayrollAppError::OperatorNotFound(operator_id.clone()),
    })
}

/// Verifies `password` against the Operator whose email folds to `email`,
/// and returns their [`OperatorId`] only when it is correct, the Operator is
/// `active`, **and** the account is not locked. `now` is the caller's clock
/// reading (issue #42's own instruction: tests pass it in rather than this
/// function sleeping on the wall clock), against which the fifteen-minute
/// window and lock are measured.
///
/// Every other outcome — no such email, a wrong password, a disabled
/// Operator, a locked account — is the single
/// [`PayrollAppError::OperatorCredentialInvalid`], deliberately carrying
/// nothing that would let a caller tell them apart (§0.12a, §0.13,
/// acceptance criteria of issues #41 and #42).
///
/// An unknown email still pays the real Argon2id cost, against
/// `DUMMY_VERIFIER`, **inside this branch** rather than short-circuited
/// before it: that is the whole timing property §0.12a describes, and
/// returning early on "no row" is exactly what would defeat it. An unknown
/// email has no row and therefore no counter, so it never locks (issue #42's
/// own instruction): a flood of unknown emails is Caddy's per-IP limit to
/// answer, not this function's.
pub async fn verify_operator_credential(
    db: &SaltDatabase,
    email: &str,
    password: &str,
    now: DateTime<Utc>,
) -> Result<OperatorId, PayrollAppError> {
    // Refused before the database is touched, and before any Argon2id work:
    // a credential this long cannot belong to any Operator, because
    // `create_operator` would have refused to record it. Answering the length
    // early costs an unauthenticated caller nothing, whereas hashing whatever
    // they sent would let them choose how much work Salt does per attempt.
    // It says nothing about any account, so it is the same refusal as every
    // other one here.
    if password.chars().count() > MAX_PASSWORD_LENGTH {
        return Err(PayrollAppError::OperatorCredentialInvalid);
    }

    type Row = (String, String, String, i32, Option<DateTime<Utc>>);

    // The row lock makes deciding whether this account is locked and changing
    // its counter one operation. Without it, a correct request and the tenth
    // failure could both read nine; the failure could commit the lock and the
    // already-approved success could then clear it and authenticate anyway.
    // Holding this one account's row while Argon2 runs deliberately serializes
    // credential attempts for that account, but not for any other Operator.
    let mut tx = db.pool().begin().await?;

    // `SET LOCAL`, so it is scoped to this transaction and released with it
    // rather than left on a pooled connection for the next use case to
    // inherit. The value is a compile-time constant of this module, never a
    // caller's, which is why it is formatted into the statement — `SET` takes
    // no bind parameters.
    sqlx::query(&format!(
        "SET LOCAL lock_timeout = {}",
        LOCK_WAIT_TIMEOUT.num_milliseconds()
    ))
    .execute(&mut *tx)
    .await?;

    let row: Result<Option<Row>, _> = sqlx::query_as(
        "SELECT id::text, password_verifier, status, failed_attempt_count, first_failure_at
         FROM operator
         WHERE lower(email) = lower($1)
         FOR UPDATE",
    )
    .bind(email.trim())
    .fetch_optional(&mut *tx)
    .await;

    let row = match row {
        Ok(row) => row,
        // Too many attempts against this one account are already queued for
        // its row. Refusing is the same opaque refusal as every other failure
        // here and leaves the counters untouched, so a caller learns nothing
        // and a queue cannot consume the pool the rest of Salt shares.
        Err(err) if is_lock_not_available(&err) => {
            return Err(PayrollAppError::OperatorCredentialInvalid);
        }
        Err(err) => return Err(err.into()),
    };

    let Some((id, password_verifier, status, failed_attempt_count, first_failure_at)) = row else {
        // `black_box` so the discarded result cannot become a reason to
        // elide the call: this line exists for its cost, and a compiler that
        // optimised it away would remove the property without removing the
        // code that claims it.
        std::hint::black_box(password_matches(password, DUMMY_VERIFIER));
        tx.commit().await?;
        return Err(PayrollAppError::OperatorCredentialInvalid);
    };

    // Still within the open window: ten or more failures inside one window
    // locks the account, and the failure that locked it restarted the window
    // at itself, so this is the lock's own fifteen minutes. It becomes false
    // again by the clock alone, with no administrator having touched the row.
    let locked = failed_attempt_count >= LOCKOUT_THRESHOLD
        && first_failure_at
            .is_some_and(|first_failure_at| now < first_failure_at + lockout_window());

    // Computed unconditionally, before the status and lock checks. Every
    // known-Operator path below also executes the same one-row UPDATE, even
    // when it deliberately leaves the counter unchanged, so lock status does
    // not add an observable database-work difference to a failed sign-in.
    let matches = password_matches(password, &password_verifier);
    let active = operator_status_from_column(&status) == OperatorStatus::Active;
    let successful = !locked && matches && active;
    let record_failure = !locked && !matches;

    // The UPDATE is deliberately unconditional for known accounts. On a
    // locked refusal or a disabled Operator's correct password it preserves
    // both columns, so neither can extend or clear a lock; it still gives
    // those refusals the same database write as a wrong password.
    let next = next_attempt_counters(
        failed_attempt_count,
        first_failure_at,
        now,
        successful,
        record_failure,
    );
    write_attempt_counters(&mut tx, &id, next).await?;
    tx.commit().await?;

    if successful {
        Ok(OperatorId::new(id))
    } else {
        Err(PayrollAppError::OperatorCredentialInvalid)
    }
}

/// The Operator row's two lockout columns: how many failures the current
/// window holds, and when that window opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AttemptCounters {
    failed_attempt_count: i32,
    first_failure_at: Option<DateTime<Utc>>,
}

/// The counter values an attempt leaves behind, decided in Rust from the
/// values read under this Operator's row lock. Pure, so the whole lockout
/// rule is provable without a database and stated in exactly one place —
/// the UPDATE that follows only writes what this returns.
///
/// A success clears the window. A counted failure either opens a new window
/// (there was none, or the previous one has closed) or extends the current
/// one. The failure that reaches [`LOCKOUT_THRESHOLD`] **restarts** the
/// window at `now`, which is what makes the lock last the full fifteen
/// minutes the issue names rather than only the remainder of a window that
/// ten slowly-spread failures may have all but used up. Any other outcome —
/// a locked account, or a disabled Operator's correct password — leaves both
/// columns exactly as they were, so neither can extend nor clear a lock.
fn next_attempt_counters(
    failed_attempt_count: i32,
    first_failure_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
    successful: bool,
    record_failure: bool,
) -> AttemptCounters {
    if successful {
        return AttemptCounters {
            failed_attempt_count: 0,
            first_failure_at: None,
        };
    }

    if !record_failure {
        return AttemptCounters {
            failed_attempt_count,
            first_failure_at,
        };
    }

    let window_is_open =
        first_failure_at.is_some_and(|opened_at| now < opened_at + lockout_window());

    let failed_attempt_count = if window_is_open {
        failed_attempt_count.saturating_add(1)
    } else {
        1
    };

    AttemptCounters {
        failed_attempt_count,
        // The failure that locks starts the lock's own fifteen minutes; a
        // failure below the threshold leaves the window it is counted in
        // where it was, so ten failures must still fall inside one window.
        first_failure_at: if !window_is_open || failed_attempt_count >= LOCKOUT_THRESHOLD {
            Some(now)
        } else {
            first_failure_at
        },
    }
}

/// Writes [`next_attempt_counters`]'s decision in the transaction holding
/// this Operator's row lock. It carries no rule of its own: both values are
/// bound, so the fifteen minutes exists only in [`lockout_window`] and
/// cannot drift into a second statement of itself here.
async fn write_attempt_counters(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    operator_id: &str,
    counters: AttemptCounters,
) -> Result<(), PayrollAppError> {
    sqlx::query(
        "UPDATE operator SET failed_attempt_count = $2, first_failure_at = $3
         WHERE id = $1::uuid",
    )
    .bind(operator_id)
    .bind(counters.failed_attempt_count)
    .bind(counters.first_failure_at)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// True when `err` is PostgreSQL refusing to keep waiting for a lock
/// (SQLSTATE 55P03), which is a `lock_timeout` elapsing and not a fact about
/// the Operator being verified. Shared with [`crate::bootstrap::bootstrap`],
/// which bounds its own table-lock wait the same way.
pub(crate) fn is_lock_not_available(err: &sqlx::Error) -> bool {
    let sqlx::Error::Database(db_err) = err else {
        return false;
    };
    db_err.code().as_deref() == Some(LOCK_NOT_AVAILABLE)
}

/// Hashes `password` to an Argon2id PHC string (§0.13), with a fresh random
/// salt per call.
fn hash_password(password: &str) -> Result<String, PayrollAppError> {
    hasher()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|err| PayrollAppError::PasswordHashingFailed(err.to_string()))
}

/// Whether `password` matches the Argon2id PHC string `phc`. A `phc` that
/// fails to parse is treated as a non-match rather than propagated: the only
/// caller with a database-backed `phc` is `verify_operator_credential`,
/// which must not distinguish a malformed row from a wrong password any
/// more than it distinguishes the other cases §0.12a names.
fn password_matches(password: &str, phc: &str) -> bool {
    let Ok(hash) = PasswordHash::new(phc) else {
        return false;
    };
    hasher().verify_password(password.as_bytes(), &hash).is_ok()
}

/// This module's Argon2id configuration, named once so hashing and
/// verifying cannot drift apart and so a later parameter change is a single
/// edit. Verification still reads its parameters from the stored PHC string
/// rather than from here, which is what lets rows hashed under older
/// parameters keep verifying after this function changes.
fn hasher() -> Argon2<'static> {
    Argon2::default()
}

/// The inverse of the `status` column's CHECK constraint. Panics rather than
/// returning a `Result`, the same discipline
/// [`crate::employer::pay_schedule_from_columns`] applies to its own CHECK:
/// disagreement here means the schema no longer matches this code, not a
/// fact about the Operator being read.
pub(crate) fn operator_status_from_column(status: &str) -> OperatorStatus {
    match status {
        "active" => OperatorStatus::Active,
        "disabled" => OperatorStatus::Disabled,
        other => panic!("operator CHECK: status is 'active' or 'disabled', found {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use argon2::{Params, Version};
    use sqlx::PgPool;

    use super::*;
    use crate::database::SaltDatabase;

    /// `OperatorId::new` is `pub(crate)`, so only a test inside this crate
    /// can hand `find_operator_by_id` an id shaped like a real Operator's
    /// that names no row — the same note `session.rs`'s and
    /// `membership.rs`'s own tests make of their own parent checks.
    #[sqlx::test]
    async fn finding_an_unknown_id_returns_none(pool: PgPool) {
        let db = SaltDatabase::from_pool(pool);
        let unknown = OperatorId::new(crate::ids::new_id());

        let found = find_operator_by_id(&db, &unknown).await.unwrap();

        assert_eq!(found, None);
    }

    /// Proves `DUMMY_VERIFIER` is itself a well-formed Argon2id PHC string
    /// that real Argon2 work can run against — the property
    /// [`verify_operator_credential`]'s unknown-email path depends on.
    #[test]
    fn the_dummy_verifier_is_a_valid_argon2id_phc_string_that_never_matches() {
        assert!(!password_matches("any password at all", DUMMY_VERIFIER));

        let parsed =
            PasswordHash::new(DUMMY_VERIFIER).expect("DUMMY_VERIFIER must parse as a PHC string");
        assert_eq!(parsed.algorithm.as_str(), "argon2id");
    }

    /// The unknown-email path is only indistinguishable from a real one
    /// while the dummy costs what a real row costs. `DUMMY_VERIFIER` is a
    /// constant, so changing [`hasher`]'s parameters without regenerating it
    /// would quietly leave the two apart and turn issue #41's timing
    /// property into the timing oracle it exists to prevent. Comparing the
    /// parameters rather than the elapsed time makes that drift a failing
    /// build on any machine.
    #[test]
    fn the_dummy_verifier_costs_what_a_real_row_costs() {
        let parsed =
            PasswordHash::new(DUMMY_VERIFIER).expect("DUMMY_VERIFIER must parse as a PHC string");

        let dummy = Params::try_from(&parsed).expect("DUMMY_VERIFIER must carry Argon2 parameters");
        let hasher = hasher();
        let current = hasher.params();

        // The three cost parameters, and only those: `output_len` reads back
        // as `Some(32)` from an encoded hash but is left `None` on the
        // configured default that produces exactly that length, so comparing
        // whole `Params` values would fail while nothing had drifted.
        assert_eq!(
            (dummy.m_cost(), dummy.t_cost(), dummy.p_cost()),
            (current.m_cost(), current.t_cost(), current.p_cost()),
            "DUMMY_VERIFIER was generated under different Argon2 parameters than this \
             module now hashes with; regenerate it against the new parameters"
        );

        assert_eq!(
            parsed.version,
            Some(Version::default() as u32),
            "DUMMY_VERIFIER names an Argon2 version this module does not hash with"
        );
    }

    #[test]
    fn a_hashed_password_verifies_against_its_own_hash_and_no_other() {
        let verifier = hash_password("correct horse battery staple").unwrap();

        assert!(password_matches("correct horse battery staple", &verifier));
        assert!(!password_matches("wrong password", &verifier));
    }

    #[test]
    fn a_malformed_stored_verifier_is_a_non_match_not_a_panic() {
        assert!(!password_matches("anything", "not a phc string"));
    }

    /// The whole lockout rule, proven without a database: the failure that
    /// reaches the threshold restarts the window at itself, so the lock that
    /// follows lasts a full [`lockout_window`] however far apart the ten
    /// failures were spread.
    #[test]
    fn the_failure_that_locks_restarts_the_window_at_itself() {
        let opened_at = DateTime::UNIX_EPOCH;
        let locked_at = opened_at + Duration::minutes(14);

        let next = next_attempt_counters(9, Some(opened_at), locked_at, false, true);

        assert_eq!(
            next,
            AttemptCounters {
                failed_attempt_count: LOCKOUT_THRESHOLD,
                first_failure_at: Some(locked_at),
            }
        );
    }

    /// A failure below the threshold is counted inside the window it arrived
    /// in and leaves that window's start where it was — otherwise every
    /// failure would push the window forward and ten of them could never
    /// fall inside one.
    #[test]
    fn a_failure_below_the_threshold_leaves_its_window_where_it_opened() {
        let opened_at = DateTime::UNIX_EPOCH;

        let inside = next_attempt_counters(
            4,
            Some(opened_at),
            opened_at + Duration::minutes(14),
            false,
            true,
        );
        assert_eq!(
            inside,
            AttemptCounters {
                failed_attempt_count: 5,
                first_failure_at: Some(opened_at),
            }
        );

        let after = next_attempt_counters(
            4,
            Some(opened_at),
            opened_at + Duration::minutes(15),
            false,
            true,
        );
        assert_eq!(
            after,
            AttemptCounters {
                failed_attempt_count: 1,
                first_failure_at: Some(opened_at + Duration::minutes(15)),
            }
        );
    }

    /// A success clears both columns; anything that is neither a success nor
    /// a counted failure — a locked account, a disabled Operator's correct
    /// password — leaves both exactly as they were.
    #[test]
    fn a_success_clears_the_window_and_an_uncounted_outcome_leaves_it_alone() {
        let opened_at = DateTime::UNIX_EPOCH;
        let now = opened_at + Duration::minutes(1);

        assert_eq!(
            next_attempt_counters(9, Some(opened_at), now, true, false),
            AttemptCounters {
                failed_attempt_count: 0,
                first_failure_at: None,
            }
        );

        assert_eq!(
            next_attempt_counters(LOCKOUT_THRESHOLD, Some(opened_at), now, false, false),
            AttemptCounters {
                failed_attempt_count: LOCKOUT_THRESHOLD,
                first_failure_at: Some(opened_at),
            }
        );
    }
}
