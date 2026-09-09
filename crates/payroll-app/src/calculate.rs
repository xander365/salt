//! `CalculatePayrollRun` (§4.9, §12): assembles a `PayrollInput` per member
//! from current facts, resolves `PayrollRules` from the `PayPeriod`'s own
//! end date, calls `calculate`, and stores input, rules and result in one
//! statement per member — overwriting whatever `WorkingPayrollCalculation`
//! was there.
//!
//! Calculation and finalization are separate, later use cases (see
//! `payroll_run.rs`'s own module doc): this file only ever writes working
//! state. It writes no `ActionLog` entry, deliberately — the
//! `WorkingPayrollCalculation` row already carries its own `calculated_at`
//! and `calculated_by`, and logging eleven recalculations while someone
//! fixes a typo would bury the entries an auditor needs.

use std::collections::HashMap;

use crate::database::SaltDatabase;
use crate::employer::pay_schedule_for_employer;
use crate::employment::get_employment_snapshot_conn;
use crate::error::PayrollAppError;
use crate::payroll_run::{
    PayrollRunId, RunStatus, active_member_ids, finalized_payrolls_for_run, lock_run,
};
use crate::unsupported_deduction_status::get_unsupported_deduction_status_conn;
use crate::year_to_date::build_year_to_date_context_conn;
use payroll::{
    EarningInstruction, EmploymentId, PayPeriod, PaySchedule, PayrollCalculation, PayrollInput,
    PayrollRules, calculate, ruleset_for,
};

/// Why one member's calculation was refused, named alongside the
/// Employment it blocked. A member with an Unknown `PriorEmployment` or
/// `UnsupportedDeductionStatus` (§4.5b, §4.5c) is the case the ticket names
/// explicitly, but any refusal `calculate` or the fact-assembly reads above
/// it can raise is reported the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayrollRunCalculationRefusal {
    pub employment_id: EmploymentId,
    pub refusal: PayrollAppError,
}

/// Recalculates every active member of `payroll_run_id`. For each one:
/// assembles a `PayrollInput` from current facts (the EmploymentSnapshot
/// and its CompensationTerms, the run's own Earning lines, a freshly built
/// `YearToDateContext`, the Employer's `PaySchedule`, and the
/// `UnsupportedDeductionStatus` in force at the period end), resolves
/// `PayrollRules` via `ruleset_for(period.end())` (ADR-0007), calls
/// `calculate`, and overwrites that member's `WorkingPayrollCalculation` —
/// input, rules and result together, in one statement, so a row can never
/// mix one calculation's input with another's result.
///
/// **A member's refusal does not stop the others.** Every active member is
/// recalculated regardless of whether an earlier one refused, and every
/// refusal is returned named by `EmploymentId` — a run with one member
/// blocked on an Unknown declaration still shows every other member's
/// current figures. A member that refuses this time has its
/// `WorkingPayrollCalculation` row cleared rather than left holding a
/// now-stale calculation that would otherwise still look current.
///
/// The run becomes `Calculated` exactly when the returned `Vec` is empty —
/// every active member has a current, successful calculation (§4.7).
/// Otherwise the run is (or remains) `Draft`. Recalculation writes no
/// `ActionLog` entry.
///
/// A run with **no** active members satisfies that condition vacuously and
/// becomes `Calculated`. This is deliberate, not an oversight: §4.7 defines
/// the state as a property of the members, and an Employer who has removed
/// everyone from a run with a stated reason for each has said something
/// complete about the period. Whether such a run may then be *finalized* is
/// finalization's own question (§5.1), asked where the history is written.
///
/// Refused outright, before any member is touched, when the run does not
/// exist or is already `Finalized` — working state can no longer change
/// once history has been written (§4.7).
pub async fn calculate_payroll_run(
    db: &SaltDatabase,
    payroll_run_id: &PayrollRunId,
    calculated_by: &str,
) -> Result<Vec<PayrollRunCalculationRefusal>, PayrollAppError> {
    let mut tx = db.pool().begin().await?;

    // Locking the run for the whole recalculation is what makes two
    // concurrent calls to this function serialize rather than race each
    // other's status update at the end. Unlike every other caller of
    // `lock_run`, this one accepts `Calculated` as well as `Draft`:
    // recalculating a run that already calculated cleanly is the ordinary
    // way to pick up a corrected fact.
    let run = lock_run(&mut tx, payroll_run_id).await?;
    if run.status == RunStatus::Finalized {
        let finalized_payrolls =
            finalized_payrolls_for_run(&mut tx, payroll_run_id, run.period.end()).await?;
        return Err(PayrollAppError::PayrollRunAlreadyFinalized {
            payroll_run_id: payroll_run_id.clone(),
            finalized_payrolls,
        });
    }
    let period = run.period;

    let schedule = pay_schedule_for_employer(&mut tx, &run.employer_id).await?;

    // Resolved once, outside the per-member loop: it depends only on the
    // period being calculated, so a missing or overlapping ruleset is a
    // catalogue problem that blocks every member identically, not a
    // per-member refusal to report and route around.
    let rules = ruleset_for(period.end())?;

    let member_ids = active_member_ids(&mut tx, payroll_run_id).await?;

    let mut earnings_by_member = run_earnings_by_member(&mut tx, payroll_run_id).await?;

    let mut refusals = Vec::new();
    for member_id in member_ids {
        let employment_id = EmploymentId::new(member_id.clone());
        let earnings = earnings_by_member.remove(&member_id).unwrap_or_default();

        match assemble_and_calculate(&mut tx, &employment_id, period, schedule, earnings, &rules)
            .await
        {
            Ok((input, calculation)) => {
                store_working_calculation(
                    &mut tx,
                    payroll_run_id,
                    &employment_id,
                    &input,
                    &rules,
                    &calculation,
                    calculated_by,
                )
                .await?;
            }
            Err(refusal) => {
                clear_working_calculation(&mut tx, payroll_run_id, &employment_id).await?;
                refusals.push(PayrollRunCalculationRefusal {
                    employment_id,
                    refusal,
                });
            }
        }
    }

    let new_status = if refusals.is_empty() {
        "calculated"
    } else {
        "draft"
    };
    sqlx::query("UPDATE payroll_run SET status = $2 WHERE id = $1::uuid")
        .bind(payroll_run_id.as_str())
        .bind(new_status)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(refusals)
}

/// Every `payroll_run_earning` row for `payroll_run_id`, in line order,
/// bucketed by `employment_id`. One query rather than one per member: the
/// table is already keyed and ordered for exactly this read.
pub(crate) async fn run_earnings_by_member(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payroll_run_id: &PayrollRunId,
) -> Result<HashMap<String, Vec<EarningInstruction>>, PayrollAppError> {
    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT employment_id, earning_json FROM payroll_run_earning
         WHERE payroll_run_id = $1::uuid ORDER BY employment_id, line",
    )
    .bind(payroll_run_id.as_str())
    .fetch_all(&mut **tx)
    .await?;

    let mut by_member: HashMap<String, Vec<EarningInstruction>> = HashMap::new();
    for (employment_id, earning_json) in rows {
        let earning: EarningInstruction = serde_json::from_value(earning_json)
            .expect("payroll_run_earning.earning_json is always a serialized EarningInstruction");
        by_member.entry(employment_id).or_default().push(earning);
    }
    Ok(by_member)
}

/// Assembles one member's `PayrollInput` from current facts and calculates
/// it — the one seam the per-member loop above branches its outcome on.
///
/// Every fact is read on the caller's own transaction, not on a second
/// connection from the pool. Two reasons, and either alone would settle it:
/// a second connection sees its own snapshot, so a `CompensationTerms`
/// correction committing mid-run could leave two members of one run
/// calculated against different facts; and holding one pooled connection
/// while asking for another is how a pool of N deadlocks under N concurrent
/// callers.
///
/// Calls the `_conn`-suffixed sibling of each read (`get_employment_snapshot_conn`,
/// not `get_employment_snapshot_on`), passing `&mut **tx` — a concrete
/// `&mut PgConnection` — rather than the generic `impl Acquire<'a>` entry
/// point every other caller of these reads uses. Issue #55 is what first
/// requires this function's own caller (`calculate_payroll_run`) to be
/// `Send`, awaited as it is from an `axum` handler; reborrowing `tx` three
/// times into a generic `impl Acquire<'a>` parameter from inside a `Send`-
/// checked call chain is a known rustc/sqlx limitation ("implementation of
/// `Acquire` is not general enough") that the concrete-typed siblings don't
/// have a generic lifetime parameter to trip.
pub(crate) async fn assemble_and_calculate(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    employment_id: &EmploymentId,
    period: PayPeriod,
    schedule: PaySchedule,
    earnings: Vec<EarningInstruction>,
    rules: &PayrollRules,
) -> Result<(PayrollInput, PayrollCalculation), PayrollAppError> {
    let employment = get_employment_snapshot_conn(tx, employment_id, period.end()).await?;
    let unsupported_deductions =
        get_unsupported_deduction_status_conn(tx, employment_id, period.end()).await?;
    let year_to_date = build_year_to_date_context_conn(tx, employment_id, period.end()).await?;

    let input = PayrollInput::new(
        employment,
        period,
        earnings,
        year_to_date,
        schedule,
        unsupported_deductions,
    );
    let calculation = calculate(&input, rules)?;
    Ok((input, calculation))
}

/// Overwrites `(payroll_run_id, employment_id)`'s `WorkingPayrollCalculation`
/// entirely — input, rules and result together in the one statement §4.9
/// requires, so a row can never mix one calculation's input with another's
/// result.
async fn store_working_calculation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payroll_run_id: &PayrollRunId,
    employment_id: &EmploymentId,
    input: &PayrollInput,
    rules: &PayrollRules,
    calculation: &PayrollCalculation,
    calculated_by: &str,
) -> Result<(), PayrollAppError> {
    let input_json = serde_json::to_value(input).expect("PayrollInput always serializes");
    let rules_json = serde_json::to_value(rules).expect("PayrollRules always serializes");
    let calculation_json =
        serde_json::to_value(calculation).expect("PayrollCalculation always serializes");

    sqlx::query(
        "INSERT INTO working_payroll_calculation
            (payroll_run_id, employment_id, payroll_input_json, payroll_rules_json,
             payroll_calculation_json, calculated_by)
         VALUES ($1::uuid, $2, $3, $4, $5, $6)
         ON CONFLICT (payroll_run_id, employment_id) DO UPDATE
         SET payroll_input_json = EXCLUDED.payroll_input_json,
             payroll_rules_json = EXCLUDED.payroll_rules_json,
             payroll_calculation_json = EXCLUDED.payroll_calculation_json,
             calculated_at = now(),
             calculated_by = EXCLUDED.calculated_by",
    )
    .bind(payroll_run_id.as_str())
    .bind(employment_id.as_str())
    .bind(input_json)
    .bind(rules_json)
    .bind(calculation_json)
    .bind(calculated_by)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Clears a member's `WorkingPayrollCalculation`, if any, when this
/// recalculation refused it — so a row from a past success can never be
/// mistaken for a current one once the facts behind it have moved on.
async fn clear_working_calculation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payroll_run_id: &PayrollRunId,
    employment_id: &EmploymentId,
) -> Result<(), PayrollAppError> {
    sqlx::query(
        "DELETE FROM working_payroll_calculation
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(payroll_run_id.as_str())
    .bind(employment_id.as_str())
    .execute(&mut **tx)
    .await?;
    Ok(())
}
