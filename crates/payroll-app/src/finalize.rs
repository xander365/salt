//! `FinalizePayrollRun` (§5, §12): the one atomic act that turns a
//! `Calculated` run into immutable history. Locks the run, then for every
//! member reassembles the `PayrollInput` from current facts, re-resolves
//! the `PayrollRules` via `ruleset_for(period.end())`, and recomputes the
//! `PayrollCalculation` — the same three paths `calculate_payroll_run`
//! already runs — and refuses unless all three equal what the
//! `WorkingPayrollCalculation` stored (§5.2, ADR-0010).
//!
//! **Comparing the calculation alone is the tempting simplification, and it
//! is wrong.** A PAYE band corrected outside the range an Employee reaches,
//! or a `CompensationTerms.effective_from` fixed without touching
//! `BasicPay`, both leave the money identical while the frozen input or
//! rules would differ — and Salt would write into permanent history an
//! explanation nobody approved and no screen ever showed. All three checks
//! run in order (input, rules, calculation) and stop at the first mismatch,
//! which is enough: finalization refuses the instant any one of the three
//! disagrees, and names which.
//!
//! All included Employments finalize, or none (§5.1): every member is
//! reassembled and compared before anything is written, and the
//! `FinalizedPayroll` rows, the liveness rows, the `PayrollFinalized`
//! `ActionLog` entry and the run's status change all commit in the one
//! transaction below.
//!
//! **The Ordinary preceding-period check (§5.3 step 3, §7)** runs once every
//! member's `employment` row is locked and before anything is rebuilt: it
//! refuses unless the immediately preceding `PayPeriod` of the same TaxYear
//! is resolved (§7.1) for every member, converting what would otherwise be
//! silent under-withholding into a visible refusal naming the Employment and
//! the unresolved period (see [`crate::sequencing`]). A `Correction` run is
//! refused outright rather than given the equivalent Correction-column check:
//! §5.3 step 3's Correction column, and the `replaces_finalized_payroll_id`
//! lineage §9 demands with it, are a later ticket's work — and a run this
//! path cannot finalize correctly must not be finalized here at all.
//!
//! Correctness rests on the run's own `FOR UPDATE` lock and the primary key
//! on `live_finalized_payroll`, not on the isolation level or an
//! application-side status check (§5.4): `READ COMMITTED` is sufficient.

use payroll::{
    EmployerId, EmploymentId, PayPeriod, PayrollCalculation, PayrollInput, PayrollRules, TaxYear,
    ruleset_for,
};
use sqlx::PgPool;

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::calculate::{assemble_and_calculate, run_earnings_by_member};
use crate::employer::pay_schedule_for_employer;
use crate::error::PayrollAppError;
use crate::ids::app_id;
use crate::payroll_run::{PayrollRunId, RunKind, RunStatus, active_member_ids, lock_run};
use crate::sequencing::verify_the_preceding_period_is_resolved_for_every_member;

/// The shape of the three JSONB snapshots this code freezes (§9, §9.1).
///
/// It ships from day one and is stored on every row, because it is the field
/// a future reader branches on to render old history without constructing
/// current domain types — and there are no in-place JSON migrations, ever
/// (the application has no `UPDATE` grant on the table). A shape change means
/// this constant becomes 2 and new rows carry 2; rows written at 1 stay at 1
/// and are still read by the version-1 reader.
pub const SNAPSHOT_SCHEMA_VERSION: i32 = 1;

app_id! {
    /// `payroll-app`'s own id for a `FinalizedPayroll` row (§4.1): a native
    /// `uuid`, minted only here, where the row itself is inserted. The only
    /// way a caller ever holds one is by receiving it back from
    /// [`finalize_payroll_run`], the same discipline [`crate::PayrollRunId`]
    /// follows.
    FinalizedPayrollId
}

/// One member's approved `WorkingPayrollCalculation`, read back so its three
/// values can be compared against a fresh reassembly/re-resolution/recompute
/// of the same three (§5.2).
struct WorkingCalculation {
    input: PayrollInput,
    rules: PayrollRules,
    calculation: PayrollCalculation,
}

/// Finalizes `payroll_run_id`: the one atomic act that rebuilds every
/// member's figures from current facts, refuses unless they still equal
/// what was approved, and — only then — writes the immutable
/// `FinalizedPayroll` history (§5).
///
/// Refused outright when the run does not exist, is already `Finalized`, or
/// is not yet `Calculated` (§4.7) — finalizing is only ever a move out of
/// `Calculated` — and when its kind is not `Ordinary`, because the
/// Correction column of §5.3 step 3 belongs to the correction ticket.
///
/// A run with no active members finalizes vacuously, for the same reason
/// `calculate_payroll_run` lets one become `Calculated`: §4.7 defines the
/// state as a property of the members, and an Employer who removed everyone
/// with a stated reason has said something complete about the period.
///
/// Returns every member's newly-minted [`FinalizedPayrollId`] alongside its
/// `EmploymentId`, empty for a vacuous run — the one place a caller can learn
/// the id a later `reverse_finalized_payroll` needs.
pub async fn finalize_payroll_run(
    pool: &PgPool,
    payroll_run_id: &PayrollRunId,
    finalized_by: &str,
) -> Result<Vec<(EmploymentId, FinalizedPayrollId)>, PayrollAppError> {
    let mut tx = pool.begin().await?;

    // The `FOR UPDATE` lock taken here is what serialises two finalizers of
    // the same run (§5.4): the loser blocks until the winner commits, then
    // re-reads `status` as `finalized` and refuses below — no
    // application-side `if status != Finalized` is doing that work.
    let run = lock_run(&mut tx, payroll_run_id).await?;
    if run.status == RunStatus::Finalized {
        return Err(PayrollAppError::PayrollRunAlreadyFinalized(
            payroll_run_id.clone(),
        ));
    }
    if run.status != RunStatus::Calculated {
        return Err(PayrollAppError::PayrollRunNotCalculated(
            payroll_run_id.clone(),
        ));
    }
    // §5.3 step 2 verifies the kind as well as the status, and this path
    // implements the Ordinary column of step 3 only. A Correction run must
    // also copy `replaces_finalized_payroll_id` from its membership row and
    // check its target is reversed and not live (§4.8, §6.3); finalizing one
    // here would write a replacement with null lineage that §9 forbids. The
    // refusal is what keeps that unreachable until the correction ticket
    // lands, rather than a comment saying it should be.
    if run.kind != RunKind::Ordinary {
        return Err(PayrollAppError::PayrollRunIsNotOrdinary(
            payroll_run_id.clone(),
        ));
    }
    // The period needs no separate verification: it is read from the locked
    // run itself and every rebuild below resolves rules, year-to-date and
    // the snapshot from that one value, so there is no second period for it
    // to disagree with.
    let period = run.period;
    let employer_id = run.employer_id;

    let schedule = pay_schedule_for_employer(&mut tx, &employer_id).await?;

    // Resolved once, outside the per-member loop, for the same reason
    // `calculate_payroll_run` resolves it once: it depends only on the
    // period, not on any one member.
    let rules = ruleset_for(period.end())?;
    let tax_year = TaxYear::for_period_end(period.end());

    let member_ids = active_member_ids(&mut tx, payroll_run_id).await?;

    // Every member's `employment` row is locked *before* any master data is
    // read (ADR-0013): `record_opening_balance` and `declare_prior_employment`
    // take `FOR UPDATE` on the same row, and `FOR SHARE` conflicts with it,
    // so an edit of a frozen fact and this finalization can never interleave.
    // Whichever arrives second waits: an edit that loses the race sees this
    // transaction's `FinalizedPayroll` and refuses; a finalization that loses
    // re-reads the edited fact below and refuses with a mismatch rather than
    // finalizing figures nobody approved. `ORDER BY id` fixes one acquisition
    // order for every finalizer, so two overlapping runs queue rather than
    // deadlock.
    lock_member_employments(&mut tx, &member_ids).await?;

    // §5.3 step 3, §7.2: an Ordinary run refuses unless the immediately
    // preceding PayPeriod of the same TaxYear is resolved for every member.
    // Run after the lock above, for the same reason master data is read
    // after it: a concurrent `OpeningBalance` or `PriorEmployment` write on
    // one of these same `employment` rows must not interleave with this
    // read (§5.4).
    verify_the_preceding_period_is_resolved_for_every_member(
        &mut tx,
        &employer_id,
        schedule,
        period,
        &member_ids,
    )
    .await?;

    let mut earnings_by_member = run_earnings_by_member(&mut tx, payroll_run_id).await?;

    // Every member is reassembled and compared before anything is written
    // (§5.1): a mismatch on the last member must leave every earlier member
    // with no `FinalizedPayroll` row either.
    let mut ready = Vec::with_capacity(member_ids.len());
    for member_id in member_ids {
        let employment_id = EmploymentId::new(member_id.clone());
        let stored = fetch_working_calculation(&mut tx, payroll_run_id, &employment_id).await?;
        let earnings = earnings_by_member.remove(&member_id).unwrap_or_default();

        // A rebuild that refuses outright — a `CompensationTerms` row deleted,
        // an Employment voided, a declaration withdrawn since the run
        // calculated — is named by the Employment it blocked, for the same
        // reason the three mismatches below are and `calculate_payroll_run`
        // returns its refusals as `PayrollRunCalculationRefusal`: an Employer
        // told only "PriorEmployment is Unknown" about a ten-member run has
        // been told nothing they can act on.
        let (current_input, current_calculation) =
            assemble_and_calculate(&mut tx, &employment_id, period, schedule, earnings, &rules)
                .await
                .map_err(|refusal| PayrollAppError::FinalizationRebuildRefused {
                    employment_id: employment_id.clone(),
                    refusal: Box::new(refusal),
                })?;

        if current_input != stored.input {
            return Err(PayrollAppError::FinalizationInputMismatch {
                employment_id,
                approved: Box::new(stored.input),
                current: Box::new(current_input),
            });
        }
        if rules != stored.rules {
            return Err(PayrollAppError::FinalizationRulesMismatch {
                employment_id,
                approved: Box::new(stored.rules),
                current: Box::new(rules),
            });
        }
        if current_calculation != stored.calculation {
            return Err(PayrollAppError::FinalizationCalculationMismatch {
                employment_id,
                approved: Box::new(stored.calculation),
                current: Box::new(current_calculation),
            });
        }

        ready.push((employment_id, current_input, current_calculation));
    }

    let mut finalized_ids = Vec::with_capacity(ready.len());
    for (employment_id, input, calculation) in ready {
        let finalized_payroll_id = insert_finalized_payroll(
            &mut tx,
            payroll_run_id,
            &employer_id,
            &employment_id,
            period,
            tax_year,
            &input,
            &rules,
            &calculation,
            finalized_by,
        )
        .await?;

        // The concurrency guard (§5.4, §6.2): a second live row for this
        // `(employment_id, period_end)` is unrepresentable, whatever the
        // `FOR UPDATE` lock above believed.
        sqlx::query(
            "INSERT INTO live_finalized_payroll (employment_id, period_end, finalized_payroll_id)
             VALUES ($1, $2, $3::uuid)",
        )
        .bind(employment_id.as_str())
        .bind(period.end())
        .bind(finalized_payroll_id.as_str())
        .execute(&mut *tx)
        .await?;

        finalized_ids.push((employment_id, finalized_payroll_id));
    }

    write_action_log_entry(
        &mut tx,
        ActionLogEntry {
            employer_id: &employer_id,
            actor: finalized_by,
            action_type: ActionType::PayrollFinalized,
            target_type: "payroll_run",
            target_id: payroll_run_id.as_str(),
            context: None,
        },
    )
    .await?;

    sqlx::query("UPDATE payroll_run SET status = 'finalized' WHERE id = $1::uuid")
        .bind(payroll_run_id.as_str())
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(finalized_ids)
}

/// Takes `FOR SHARE` on each member's `employment` row, in one statement and
/// in a fixed order. Reading the ids back is what proves the lock was taken:
/// a member whose Employment vanished between the membership read and this
/// one is impossible — `employment` is never physically deleted (§4.3) — so
/// a short result would mean the schema no longer matches this code.
async fn lock_member_employments(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    member_ids: &[String],
) -> Result<(), PayrollAppError> {
    let locked: Vec<String> =
        sqlx::query_scalar("SELECT id FROM employment WHERE id = ANY($1) ORDER BY id FOR SHARE")
            .bind(member_ids)
            .fetch_all(&mut **tx)
            .await?;

    assert_eq!(
        locked.len(),
        member_ids.len(),
        "an Employment is never physically deleted (§4.3), so every active \
         member of a run still has a row to lock"
    );
    Ok(())
}

/// Reads back one member's approved `WorkingPayrollCalculation`. A missing
/// row here would mean the run's own `Calculated` status lied — every active
/// member has one by definition (§4.7) — and the run lock held since before
/// this function was called rules out a membership or calculation change
/// slipping in underneath it.
async fn fetch_working_calculation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payroll_run_id: &PayrollRunId,
    employment_id: &EmploymentId,
) -> Result<WorkingCalculation, PayrollAppError> {
    let (input_json, rules_json, calculation_json): (
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
    ) = sqlx::query_as(
        "SELECT payroll_input_json, payroll_rules_json, payroll_calculation_json
         FROM working_payroll_calculation
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(payroll_run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&mut **tx)
    .await?;

    Ok(WorkingCalculation {
        input: serde_json::from_value(input_json).expect(
            "working_payroll_calculation.payroll_input_json is always a serialized PayrollInput",
        ),
        rules: serde_json::from_value(rules_json).expect(
            "working_payroll_calculation.payroll_rules_json is always a serialized PayrollRules",
        ),
        calculation: serde_json::from_value(calculation_json).expect(
            "working_payroll_calculation.payroll_calculation_json is always a serialized PayrollCalculation",
        ),
    })
}

/// Inserts one immutable `FinalizedPayroll` row, freezing the complete
/// `PayrollInput`, `PayrollRules` and `PayrollCalculation`, both rule ids,
/// the `SaltVersion`, and `TaxableRemuneration`/`PAYE` as real numeric
/// columns beside the JSONB snapshots (§9) — the columns year-to-date
/// actually reads.
///
/// `snapshot_schema_version` is bound explicitly from
/// [`SNAPSHOT_SCHEMA_VERSION`] rather than left to the column's `DEFAULT 1`
/// (migration 0009). §9.1 makes it the field a future reader branches on to
/// render old history, and there are no in-place JSON migrations ever — so
/// the version must be the one *this code's* snapshot shape actually is. A
/// default states what the column was created with, which stops being the
/// same fact the day a shape change ships.
#[allow(clippy::too_many_arguments)]
async fn insert_finalized_payroll(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payroll_run_id: &PayrollRunId,
    employer_id: &EmployerId,
    employment_id: &EmploymentId,
    period: PayPeriod,
    tax_year: TaxYear,
    input: &PayrollInput,
    rules: &PayrollRules,
    calculation: &PayrollCalculation,
    finalized_by: &str,
) -> Result<FinalizedPayrollId, PayrollAppError> {
    let input_json = serde_json::to_value(input).expect("PayrollInput always serializes");
    let rules_json = serde_json::to_value(rules).expect("PayrollRules always serializes");
    let calculation_json =
        serde_json::to_value(calculation).expect("PayrollCalculation always serializes");

    let id: String = sqlx::query_scalar(
        "INSERT INTO finalized_payroll
            (payroll_run_id, employment_id, employer_id, period_start, period_end, tax_year,
             payroll_input_json, payroll_rules_json, payroll_calculation_json,
             taxable_remuneration, paye, paye_table_id, ssc_rules_id, salt_version,
             snapshot_schema_version, finalized_by)
         VALUES ($1::uuid, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
         RETURNING id::text",
    )
    .bind(payroll_run_id.as_str())
    .bind(employment_id.as_str())
    .bind(employer_id.as_str())
    .bind(period.start())
    .bind(period.end())
    .bind(tax_year.starting_year())
    .bind(input_json)
    .bind(rules_json)
    .bind(calculation_json)
    .bind(calculation.taxable_remuneration.cents())
    .bind(calculation.paye.amount.cents())
    .bind(calculation.paye_table_id.as_str())
    .bind(calculation.ssc_rules_id.as_str())
    .bind(crate::SALT_VERSION)
    .bind(SNAPSHOT_SCHEMA_VERSION)
    .bind(finalized_by)
    .fetch_one(&mut **tx)
    .await?;
    Ok(FinalizedPayrollId::new(id))
}
