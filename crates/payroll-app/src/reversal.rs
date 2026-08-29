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

use payroll::EmployerId;
use sqlx::PgPool;

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::error::PayrollAppError;
use crate::finalize::FinalizedPayrollId;

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
/// `reversal.finalized_payroll_id` is UNIQUE (migration 0011); that
/// constraint, not this check, is what resolves two genuinely concurrent
/// reversals of the same row to one (§11) — the same split
/// `finalize_payroll_run`'s own concurrency guarantee rests on.
pub async fn reverse_finalized_payroll(
    pool: &PgPool,
    finalized_payroll_id: &FinalizedPayrollId,
    reason: &str,
    reversed_by: &str,
) -> Result<(), PayrollAppError> {
    if reason.trim().is_empty() {
        return Err(PayrollAppError::ReversalReasonCannotBeEmpty);
    }

    let mut tx = pool.begin().await?;

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

    // The `NOT EXISTS` guard is what turns the common, single-threaded
    // double-reversal into this named refusal rather than a raw constraint
    // violation. The UNIQUE constraint on `reversal.finalized_payroll_id`
    // stands regardless, and is what a genuine race between two reversals
    // actually resolves on (§11).
    let inserted = sqlx::query(
        "INSERT INTO reversal (finalized_payroll_id, reversed_by, reason)
         SELECT $1::uuid, $2, $3
         WHERE NOT EXISTS (
             SELECT 1 FROM reversal WHERE finalized_payroll_id = $1::uuid
         )",
    )
    .bind(finalized_payroll_id.as_str())
    .bind(reversed_by)
    .bind(reason)
    .execute(&mut *tx)
    .await?;
    if inserted.rows_affected() == 0 {
        return Err(PayrollAppError::FinalizedPayrollAlreadyReversed(
            finalized_payroll_id.clone(),
        ));
    }

    // The deleted liveness row is not history — the `Reversal` row just
    // inserted is (§6.2). This delete is the whole reason a later-built
    // `YearToDateContext` drops the period: "live" means joined through
    // this table, and that query never mentions `reversal` (§8).
    sqlx::query("DELETE FROM live_finalized_payroll WHERE finalized_payroll_id = $1::uuid")
        .bind(finalized_payroll_id.as_str())
        .execute(&mut *tx)
        .await?;

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
