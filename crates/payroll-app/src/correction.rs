//! `AddEmploymentToCorrectionRun` (§4.8, §4.5d, §6.3, ADR-0015): the one act
//! that gives a Correction run its single member, its declared replacement
//! target (if any), and its pre-populated Earning lines.
//!
//! Whether a null target is *legitimate* — a reasoned removal, or the
//! Employment never having been a member — is a question only finalization
//! can answer safely (§5.3 step 3), because nothing before that point can
//! promise the answer will not change: an Ordinary run for the same period
//! can still finalize, or still reasonedly remove the Employment, in
//! between. [`verify_null_lineage_is_legitimate`] is called from
//! `finalize.rs`, not from here.

use payroll::{Earning, EmployerId, EmploymentId, PayPeriod};
use sqlx::PgPool;

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::error::PayrollAppError;
use crate::finalize::{FinalizedPayrollId, SNAPSHOT_SCHEMA_VERSION};
use crate::payroll_run::{LockedRun, PayrollRunId, RunKind, lock_and_reopen_run};

/// What happened to a Correction run's Earning lines when
/// [`add_employment_to_correction_run`] tried to pre-populate them from a
/// reversed `FinalizedPayroll`'s frozen snapshot (§4.5d, §6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EarningPrePopulation {
    /// No target was named, so there is no snapshot to read from; the run
    /// starts with no Earning lines, the same as a fresh Ordinary member.
    NoTarget,
    /// `count` Earning lines were copied from the target's frozen
    /// `PayrollInput`.
    FromTarget { count: usize },
    /// The target's `snapshot_schema_version` is not one this build of Salt
    /// deserializes. The run starts with no Earning lines rather than
    /// guessing at an unreadable shape (§9.1) — this variant is how that
    /// degradation "says so" to the caller.
    UnreadableSnapshot { schema_version: i32 },
}

/// Adds `employment_id` as the single member of Correction `payroll_run_id`,
/// declaring `replaces` as the `FinalizedPayroll` it names as its target —
/// or `None` for the two null-lineage cases §4.8 allows. Whether the run may
/// actually *finalize* with a null target is checked later, in
/// `finalize_payroll_run` — see [`verify_null_lineage_is_legitimate`].
///
/// Refused when the run is not a Correction, already has a member (§4.8,
/// ADR-0015 — "one Employment rather than several"), or when
/// `employment_id` is void, not found, or belongs to a different Employer.
///
/// When `replaces` is given, it is checked immediately: it must exist, name
/// this Employment, this Employer and this run's own period end, and carry a
/// `Reversal` (§4.8 — "the target must be reversed and not Live"). A
/// reversed `FinalizedPayroll`'s liveness row is always gone —
/// `reverse_finalized_payroll` deletes it in the same transaction that
/// inserts the `Reversal` — so "carries a `Reversal`" and "is not Live" are
/// one fact to check, not two.
///
/// Earning lines are then pre-populated from the target's frozen
/// `PayrollInput`, degrading to none — and saying so through the returned
/// [`EarningPrePopulation`] — when its `snapshot_schema_version` is not one
/// this build reads (§4.5d, §9.1). The rest of a Correction's
/// `PayrollInput` is assembled fresh from current master data by the
/// ordinary calculation path (§6.3, §6.5); Earning lines are the one field
/// with no other source.
pub async fn add_employment_to_correction_run(
    pool: &PgPool,
    payroll_run_id: &PayrollRunId,
    employment_id: &EmploymentId,
    replaces: Option<&FinalizedPayrollId>,
    actor: &str,
) -> Result<EarningPrePopulation, PayrollAppError> {
    let mut tx = pool.begin().await?;

    let run = lock_and_reopen_run(&mut tx, payroll_run_id).await?;
    if run.kind != RunKind::Correction {
        return Err(PayrollAppError::PayrollRunIsNotCorrection(
            payroll_run_id.clone(),
        ));
    }

    let already_has_a_member: Option<bool> = sqlx::query_scalar(
        "SELECT TRUE FROM payroll_run_employment WHERE payroll_run_id = $1::uuid",
    )
    .bind(payroll_run_id.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    if already_has_a_member.is_some() {
        return Err(PayrollAppError::CorrectionRunAlreadyHasAnEmployment(
            payroll_run_id.clone(),
        ));
    }

    let employment_row: Option<(String, bool)> =
        sqlx::query_as("SELECT employer_id, is_void FROM employment WHERE id = $1")
            .bind(employment_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
    let Some((employment_employer_id, is_void)) = employment_row else {
        return Err(PayrollAppError::EmploymentNotFound(employment_id.clone()));
    };
    if is_void {
        return Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()));
    }
    if employment_employer_id != run.employer_id.as_str() {
        return Err(
            PayrollAppError::CorrectionEmploymentBelongsToADifferentEmployer {
                employment_id: employment_id.clone(),
                employer_id: run.employer_id.clone(),
            },
        );
    }

    let target_snapshot = match replaces {
        Some(target) => Some(
            validate_correction_target(&mut tx, payroll_run_id, employment_id, &run, target)
                .await?,
        ),
        None => None,
    };

    sqlx::query(
        "INSERT INTO payroll_run_employment
            (payroll_run_id, employment_id, replaces_finalized_payroll_id)
         VALUES ($1::uuid, $2, $3::uuid)",
    )
    .bind(payroll_run_id.as_str())
    .bind(employment_id.as_str())
    .bind(replaces.map(FinalizedPayrollId::as_str))
    .execute(&mut *tx)
    .await?;

    let pre_population = match target_snapshot {
        Some((schema_version, input_json)) => {
            prepopulate_earnings(
                &mut tx,
                payroll_run_id,
                employment_id,
                schema_version,
                &input_json,
            )
            .await?
        }
        None => EarningPrePopulation::NoTarget,
    };

    write_action_log_entry(
        &mut tx,
        ActionLogEntry {
            employer_id: &run.employer_id,
            actor,
            action_type: ActionType::EmploymentAddedToCorrectionRun,
            target_type: "employment",
            target_id: employment_id.as_str(),
            context: Some(serde_json::json!({
                "correction_reason": run.correction_reason,
                "replaces_finalized_payroll_id": replaces.map(FinalizedPayrollId::as_str),
            })),
        },
    )
    .await?;

    tx.commit().await?;
    Ok(pre_population)
}

/// Checks `target` is a legitimate declared target for `employment_id` in
/// Correction `payroll_run_id` — exists, names this Employment, this
/// Employer and this run's own period end, and carries a `Reversal` (§4.8)
/// — and returns its frozen snapshot version and input for
/// [`prepopulate_earnings`] to read.
async fn validate_correction_target(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payroll_run_id: &PayrollRunId,
    employment_id: &EmploymentId,
    run: &LockedRun,
    target: &FinalizedPayrollId,
) -> Result<(i32, serde_json::Value), PayrollAppError> {
    let target_row: Option<(String, String, chrono::NaiveDate, i32, serde_json::Value)> =
        sqlx::query_as(
            "SELECT employment_id, employer_id, period_end, snapshot_schema_version,
                    payroll_input_json
             FROM finalized_payroll WHERE id = $1::uuid",
        )
        .bind(target.as_str())
        .fetch_optional(&mut **tx)
        .await?;
    let Some((
        target_employment_id,
        target_employer_id,
        target_period_end,
        schema_version,
        input_json,
    )) = target_row
    else {
        return Err(PayrollAppError::FinalizedPayrollNotFound(target.clone()));
    };
    if target_employment_id != employment_id.as_str()
        || target_employer_id != run.employer_id.as_str()
        || target_period_end != run.period.end()
    {
        return Err(PayrollAppError::CorrectionTargetDoesNotMatch {
            payroll_run_id: payroll_run_id.clone(),
            finalized_payroll_id: target.clone(),
        });
    }

    let is_reversed: Option<bool> =
        sqlx::query_scalar("SELECT TRUE FROM reversal WHERE finalized_payroll_id = $1::uuid")
            .bind(target.as_str())
            .fetch_optional(&mut **tx)
            .await?;
    if is_reversed.is_none() {
        return Err(PayrollAppError::CorrectionTargetNotReversed(target.clone()));
    }

    Ok((schema_version, input_json))
}

/// Copies `input_json`'s `earnings` array into `payroll_run_earning` rows
/// for `(payroll_run_id, employment_id)`, when `schema_version` is one this
/// build reads. Only the `earnings` field is read — never the whole frozen
/// `PayrollInput` — because that is the only field this pre-population
/// exists to carry forward (§4.5d); the rest of a Correction's
/// `PayrollInput` comes fresh from current master data (§6.5).
async fn prepopulate_earnings(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payroll_run_id: &PayrollRunId,
    employment_id: &EmploymentId,
    schema_version: i32,
    input_json: &serde_json::Value,
) -> Result<EarningPrePopulation, PayrollAppError> {
    if schema_version != SNAPSHOT_SCHEMA_VERSION {
        return Ok(EarningPrePopulation::UnreadableSnapshot { schema_version });
    }

    let earnings: Vec<Earning> = match input_json.get("earnings") {
        Some(earnings_json) => serde_json::from_value(earnings_json.clone()).expect(
            "a snapshot at the current SNAPSHOT_SCHEMA_VERSION always carries a readable \
             earnings array",
        ),
        None => Vec::new(),
    };

    for (index, earning) in earnings.iter().enumerate() {
        let line = i16::try_from(index)
            .expect("a payroll run holds far fewer than i16::MAX earning lines");
        let earning_json = serde_json::to_value(earning).expect("Earning always serializes");
        sqlx::query(
            "INSERT INTO payroll_run_earning (payroll_run_id, employment_id, line, earning_json)
             VALUES ($1::uuid, $2, $3, $4)",
        )
        .bind(payroll_run_id.as_str())
        .bind(employment_id.as_str())
        .bind(line)
        .bind(earning_json)
        .execute(&mut **tx)
        .await?;
    }

    Ok(EarningPrePopulation::FromTarget {
        count: earnings.len(),
    })
}

/// Whether a Correction run's single member may legitimately finalize with
/// no declared target (§4.8's two null-lineage cases): the Employment was
/// removed with a reason from the finalized Ordinary run for `period`, or it
/// was never a member of that run at all.
///
/// Called from `finalize_payroll_run`, not from
/// [`add_employment_to_correction_run`] — nothing before finalization can
/// promise a `Draft` run's answer will not change, so the check has to run
/// inside the finalization transaction itself (§5.3 step 3), under the same
/// member lock finalization already holds.
pub(crate) async fn verify_null_lineage_is_legitimate(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    employer_id: &EmployerId,
    employment_id: &EmploymentId,
    period: PayPeriod,
) -> Result<(), PayrollAppError> {
    let membership: Option<(bool, String)> = sqlx::query_as(
        "SELECT payroll_run_employment.removed_at IS NOT NULL, payroll_run.status
         FROM payroll_run_employment
         JOIN payroll_run ON payroll_run.id = payroll_run_employment.payroll_run_id
         WHERE payroll_run_employment.employment_id = $1
           AND payroll_run.employer_id = $2
           AND payroll_run.period_end = $3
           AND payroll_run.kind = 'ordinary'",
    )
    .bind(employment_id.as_str())
    .bind(employer_id.as_str())
    .bind(period.end())
    .fetch_optional(&mut **tx)
    .await?;

    // `None` — never a member of the Ordinary run for this period at all
    // (§4.8's second case). `Some` — a member of it; legitimate only when
    // removed with a reason from a run that went on to finalize (§4.8's
    // first case) — a removal from a run still Draft or Calculated says
    // nothing yet, the same reason `sequencing.rs` gives for its own
    // `removed_with_a_reason` read.
    let legitimate = match membership {
        None => true,
        Some((removed_with_a_reason, status)) => removed_with_a_reason && status == "finalized",
    };

    if legitimate {
        Ok(())
    } else {
        Err(PayrollAppError::CorrectionLineageNotLegitimate {
            employment_id: employment_id.clone(),
            period,
        })
    }
}
