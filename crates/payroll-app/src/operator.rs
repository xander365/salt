//! `CreateOperator`, `FindOperatorByEmail`, `DisableOperator` and
//! `VerifyOperatorCredential` — the global human identity Salt can name
//! (issue #41, parent #38;
//! `docs/domain/operator-auth-http-web-grill.md` §6, §9, §0.12a-§0.13).
//!
//! An Operator is authenticated here; authorizing one against an Employer
//! through `EmployerMembership` is a later spec's table, not this one's.

use argon2::password_hash::phc::PasswordHash;
use argon2::{Argon2, PasswordHasher, PasswordVerifier};

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

/// A fixed, valid Argon2id PHC string, generated once offline against this
/// module's own `Argon2::default()` parameters and a fixed salt — never
/// regenerated at runtime. It verifies no real Operator's password; its only
/// purpose is to give an unknown email in [`verify_operator_credential`] the
/// same Argon2id work a real row costs (§0.12a: "An unknown email does the
/// same Argon2id work against a fixed dummy verifier, so the response time
/// does not separate 'no such account' from 'wrong password'").
const DUMMY_VERIFIER: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdGR1bW15dmVyaWZpZXJzYWx0Zm9yNDF4eA$\
     6gfr+ZD2aJP77k3KUWJnrPmvgefDG4meLGL0DMD7Oy8";

/// Password limits required by the credential-handling design. Length is
/// measured in Unicode scalar values, so a password's limit is not changed
/// merely because it contains non-ASCII characters.
const MIN_PASSWORD_LENGTH: usize = 8;
const MAX_PASSWORD_LENGTH: usize = 1024;

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
/// `email` is stored exactly as given; a second Operator whose email folds
/// to the same lower-cased form is refused by the database's own unique
/// index, not by an application-side check racing it (see
/// `OPERATOR_EMAIL_FOLDED_KEY`).
pub async fn create_operator(
    db: &SaltDatabase,
    email: &str,
    display_name: &str,
    password: &str,
) -> Result<OperatorId, PayrollAppError> {
    if email.trim().is_empty() {
        return Err(PayrollAppError::OperatorEmailCannotBeEmpty);
    }
    if display_name.trim().is_empty() {
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
    .execute(db.pool())
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
    .bind(email)
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
/// and returns their [`OperatorId`] only when it is correct **and** the
/// Operator is `active`.
///
/// Every other outcome — no such email, a wrong password, a disabled
/// Operator — is the single [`PayrollAppError::OperatorCredentialInvalid`],
/// deliberately carrying nothing that would let a caller tell the three
/// apart (§0.12a, §0.13, acceptance criteria of issue #41).
///
/// An unknown email still pays the real Argon2id cost, against
/// `DUMMY_VERIFIER`, **inside this branch** rather than short-circuited
/// before it: that is the whole timing property §0.12a describes, and
/// returning early on "no row" is exactly what would defeat it.
pub async fn verify_operator_credential(
    db: &SaltDatabase,
    email: &str,
    password: &str,
) -> Result<OperatorId, PayrollAppError> {
    type Row = (String, String, String);

    let row: Option<Row> = sqlx::query_as(
        "SELECT id::text, password_verifier, status FROM operator
         WHERE lower(email) = lower($1)",
    )
    .bind(email)
    .fetch_optional(db.pool())
    .await?;

    let Some((id, password_verifier, status)) = row else {
        password_matches(password, DUMMY_VERIFIER);
        return Err(PayrollAppError::OperatorCredentialInvalid);
    };

    // Computed unconditionally, before the status check: a disabled
    // Operator's row must cost exactly what an active one's does, or the
    // response time itself would say "this account exists and is disabled".
    let matches = password_matches(password, &password_verifier);
    if !matches || status != "active" {
        return Err(PayrollAppError::OperatorCredentialInvalid);
    }

    Ok(OperatorId::new(id))
}

/// Hashes `password` to an Argon2id PHC string (§0.13), with a fresh random
/// salt per call.
fn hash_password(password: &str) -> Result<String, PayrollAppError> {
    Argon2::default()
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
    Argon2::default()
        .verify_password(password.as_bytes(), &hash)
        .is_ok()
}

/// The inverse of the `status` column's CHECK constraint. Panics rather than
/// returning a `Result`, the same discipline
/// [`crate::employer::pay_schedule_from_columns`] applies to its own CHECK:
/// disagreement here means the schema no longer matches this code, not a
/// fact about the Operator being read.
fn operator_status_from_column(status: &str) -> OperatorStatus {
    match status {
        "active" => OperatorStatus::Active,
        "disabled" => OperatorStatus::Disabled,
        other => panic!("operator CHECK: status is 'active' or 'disabled', found {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
