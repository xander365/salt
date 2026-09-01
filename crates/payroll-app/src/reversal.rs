//! `ReverseFinalizedPayroll` (§6.1, §12): the one act that turns a live
//! `FinalizedPayroll` into reversed history, with a mandatory reason and an
//! attributed actor and time. The original `FinalizedPayroll` row is never
//! touched — `REVOKE UPDATE, DELETE` on that table (migration 0015) makes
//! that a database permission, not application discipline. This use case
//! only ever deletes the liveness row and inserts an immutable `Reversal`.
//!
//! **A reversal cannot be undone.** There is no unreversal record type, by
//! design (§6.1): a mistaken reversal is repaired by finalizing a
//! replacement identical to the original — a later, separate ticket. That
//! design is enforced here simply by there being no function that does it.

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::database::SaltDatabase;
use crate::error::PayrollAppError;
use crate::finalize::FinalizedPayrollId;
use payroll::EmployerId;

/// Reverses `finalized_payroll_id`: deletes its `live_finalized_payroll` row
/// so a `YearToDateContext` built afterwards drops the period immediately
/// (§6.3), records an immutable `Reversal` naming `reversed_by`, the time,
/// and the mandatory `reason`, and writes the `FinalizedPayrollReversed`
/// `ActionLog` entry — all in the one transaction below (§10).
///
/// Demands a `reason` that is not blank, checked before anything is written,
/// for the same reason `remove_employment_from_run` does: a reversal is a
/// deliberate, attributed act, and a reason of `" "` is one nobody can read
/// six months later in a log that cannot afterwards be corrected.
///
/// Refused when `finalized_payroll_id` does not exist, or already has a
/// `Reversal` (§6.1) — a `FinalizedPayroll` can be reversed only once.
/// `reversal.finalized_payroll_id` is UNIQUE (migration 0011), and that one
/// constraint answers both the everyday second attempt and two genuinely
/// concurrent reversals of the same row (§11). The loser of either reads the
/// same named refusal, so a race is never the caller's problem to tell apart
/// from a repeat.
pub async fn reverse_finalized_payroll(
    db: &SaltDatabase,
    finalized_payroll_id: &FinalizedPayrollId,
    reason: &str,
    reversed_by: &str,
) -> Result<(), PayrollAppError> {
    if reason.trim().is_empty() {
        return Err(PayrollAppError::ReversalReasonCannotBeEmpty);
    }

    let mut tx = db.pool().begin().await?;

    let target: Option<(String,)> =
        sqlx::query_as("SELECT employer_id FROM finalized_payroll WHERE id = $1::uuid")
            .bind(finalized_payroll_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
    let Some((employer_id,)) = target else {
        return Err(PayrollAppError::FinalizedPayrollNotFound(
            finalized_payroll_id.clone(),
        ));
    };
    let employer_id = EmployerId::new(employer_id);

    // "Reversed only once" is the UNIQUE constraint on
    // `reversal.finalized_payroll_id`, and nothing else. An application-side
    // `SELECT … WHERE NOT EXISTS` cannot see a concurrent transaction's
    // uncommitted row, so it would refuse the everyday second attempt while
    // leaving the loser of a genuine race with a raw constraint violation —
    // two errors for one refusal. Reading the violation back into
    // `FinalizedPayrollAlreadyReversed` gives both callers the same answer
    // (§11).
    let insert = sqlx::query(
        "INSERT INTO reversal (finalized_payroll_id, reversed_by, reason)
         VALUES ($1::uuid, $2, $3)",
    )
    .bind(finalized_payroll_id.as_str())
    .bind(reversed_by)
    .bind(reason)
    .execute(&mut *tx)
    .await;
    if let Err(err) = insert {
        if is_unique_violation(&err, REVERSAL_ONE_PER_FINALIZED_PAYROLL) {
            return Err(PayrollAppError::FinalizedPayrollAlreadyReversed(
                finalized_payroll_id.clone(),
            ));
        }
        return Err(err.into());
    }

    // The deleted liveness row is not history — the `Reversal` row just
    // inserted is (§6.2). This delete is the whole reason a later-built
    // `YearToDateContext` drops the period: "live" means joined through
    // this table, and that query never mentions `reversal` (§8).
    let deleted =
        sqlx::query("DELETE FROM live_finalized_payroll WHERE finalized_payroll_id = $1::uuid")
            .bind(finalized_payroll_id.as_str())
            .execute(&mut *tx)
            .await?;
    assert_eq!(
        deleted.rows_affected(),
        1,
        "a FinalizedPayroll that has no Reversal is live: finalization inserts \
         its liveness row, and only this function ever deletes one. Reaching \
         here having deleted nothing means some later use case took liveness \
         away without recording a Reversal, which §6.2 does not allow."
    );

    write_action_log_entry(
        &mut tx,
        ActionLogEntry {
            employer_id: &employer_id,
            actor: reversed_by,
            action_type: ActionType::FinalizedPayrollReversed,
            target_type: "finalized_payroll",
            target_id: finalized_payroll_id.as_str(),
            context: Some(serde_json::json!({ "reason": reason })),
        },
    )
    .await?;

    tx.commit().await?;
    Ok(())
}

/// The name PostgreSQL gives migration 0011's `UNIQUE (finalized_payroll_id)`
/// on `reversal` — the constraint that makes "reversed only once" true.
const REVERSAL_ONE_PER_FINALIZED_PAYROLL: &str = "reversal_finalized_payroll_id_key";

/// True when `err` is PostgreSQL's unique violation (SQLSTATE 23505) raised
/// by `constraint`. Named rather than matched on the message text, so a
/// different unique constraint failing here is still reported as the database
/// error it is, never mistaken for this one.
fn is_unique_violation(err: &sqlx::Error, constraint: &str) -> bool {
    let sqlx::Error::Database(db_err) = err else {
        return false;
    };
    db_err.code().as_deref() == Some("23505") && db_err.constraint() == Some(constraint)
}
