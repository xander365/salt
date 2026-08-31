//! `RecordCompensationTerms` and `CorrectCompensationTerms` (§4.4, §6.5,
//! §12). Recording is a bare insert of a new row; correcting changes an
//! existing one — at any time, including where Live finalized payroll
//! references it — because §6.5 makes a correction a payroll act that
//! gathers a reason and computes a divergence list against live finalized
//! periods, not something a bare record call can do.

use chrono::NaiveDate;
use payroll::{EmploymentId, Money, PayPeriod, validate_effective_from_is_a_period_start};
use sqlx::PgPool;

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::employer::lock_the_pay_schedule_governing;
use crate::error::PayrollAppError;
use crate::freeze::live_finalized_periods_in_span;

/// Records a `CompensationTerms` row effective from `effective_from`. Refused
/// as a domain refusal, not a database error, when `effective_from` is not a
/// start date of one of the Employer's `PaySchedule`'s own `PayPeriod`s
/// (INV-014) — checked in Rust before the row is written, using the pure
/// crate's `PaySchedule` rather than reimplementing its month arithmetic
/// here.
///
/// Refused too when the Employment is void (§4.3): a voided Employment is a
/// recorded mistake that never reaches a payroll, so standing pay facts
/// against it would describe an Employment nobody will ever pay.
pub async fn record_compensation_terms(
    pool: &PgPool,
    employment_id: &EmploymentId,
    effective_from: NaiveDate,
    basic_pay: Money,
    created_by: &str,
) -> Result<(), PayrollAppError> {
    let mut tx = pool.begin().await?;

    // The Employer row is locked first, and `FOR SHARE` is what makes the
    // `effective_from` guard below hold: `change_pay_schedule` takes `FOR
    // UPDATE` on that row and reads this table looking for an
    // `effective_from` its new schedule would strand, so without the lock a
    // schedule change and this write neither conflict nor see each other,
    // and both commit — leaving a row claiming a rise took effect on a day
    // that is no longer the start of anything (INV-014).
    let Some((_, schedule)) = lock_the_pay_schedule_governing(&mut tx, employment_id).await? else {
        return Err(PayrollAppError::EmploymentNotFound(employment_id.clone()));
    };

    // `FOR SHARE OF employment` holds the row against a concurrent
    // `void_employment`, whose `UPDATE` needs the exclusive lock. Without
    // it, a void committing between this read and the insert would leave a
    // CompensationTerms row recorded against a voided Employment.
    let is_void: Option<bool> =
        sqlx::query_scalar("SELECT is_void FROM employment WHERE id = $1 FOR SHARE")
            .bind(employment_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
    let is_void =
        is_void.ok_or_else(|| PayrollAppError::EmploymentNotFound(employment_id.clone()))?;
    if is_void {
        return Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()));
    }

    validate_effective_from_is_a_period_start(schedule, effective_from)?;

    sqlx::query(
        "INSERT INTO compensation_terms (employment_id, effective_from, basic_pay, created_by)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(employment_id.as_str())
    .bind(effective_from)
    .bind(basic_pay.cents())
    .bind(created_by)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(())
}

/// Corrects the existing `CompensationTerms` row identified by
/// `(employment_id, current_effective_from)` to `new_effective_from` and
/// `new_basic_pay` — at any time, including where Live finalized payroll
/// references it (§6.5, ADR-0013). Recording a brand new row is
/// [`record_compensation_terms`]; this is the "separate, later use case"
/// that module doc named for correcting one that already exists.
///
/// Demands a non-empty `reason` (refused otherwise, before anything is
/// touched) and writes a `CompensationTermsCorrected` `ActionLog` entry
/// carrying it, the before and after `(effective_from, basic_pay)`, and the
/// divergence list below — the three things §6.5 guard 1 requires of every
/// correction.
///
/// Returns every Live finalized `PayPeriod` the row **currently** governs:
/// from `current_effective_from` up to whichever later row's own
/// `effective_from` already bounds it, or open-ended when none exists.
/// Computed *before* the row is updated, so the list names exactly what this
/// correction is about to disagree with — never a refusal (§6.5 guard 2),
/// and it reads `finalized_payroll` and `live_finalized_payroll` without
/// writing either (§6.5 guard 3).
///
/// Splitting a row (§6.5's March/April/May case) is two separate calls, in
/// this order: [`record_compensation_terms`] inserts the new row first
/// (unconstrained, since it is new data, not a correction of a fact live
/// payroll relied on), then this call corrects the *existing* row's
/// `effective_from` forward. Read in that order, this function's divergence
/// list still names every period the *original*, wider span covered — an
/// insert never narrows another row's already-computed "next effective_from"
/// answer for a call already past that read.
pub async fn correct_compensation_terms(
    pool: &PgPool,
    employment_id: &EmploymentId,
    current_effective_from: NaiveDate,
    new_effective_from: NaiveDate,
    new_basic_pay: Money,
    reason: &str,
    corrected_by: &str,
) -> Result<Vec<PayPeriod>, PayrollAppError> {
    if reason.trim().is_empty() {
        return Err(PayrollAppError::CompensationTermsCorrectionReasonCannotBeEmpty);
    }

    let mut tx = pool.begin().await?;

    // Same lock, same reason as `record_compensation_terms`: it is what
    // makes the INV-014 check on `new_effective_from` below hold against a
    // concurrent `change_pay_schedule`.
    let Some((employer_id, schedule)) =
        lock_the_pay_schedule_governing(&mut tx, employment_id).await?
    else {
        return Err(PayrollAppError::EmploymentNotFound(employment_id.clone()));
    };

    let is_void: Option<bool> =
        sqlx::query_scalar("SELECT is_void FROM employment WHERE id = $1 FOR SHARE")
            .bind(employment_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
    let is_void =
        is_void.ok_or_else(|| PayrollAppError::EmploymentNotFound(employment_id.clone()))?;
    if is_void {
        return Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()));
    }

    validate_effective_from_is_a_period_start(schedule, new_effective_from)?;

    // `FOR UPDATE` holds the row against a concurrent correction of the same
    // fact while this one computes its divergence list and decides the new
    // values, so two corrections of one row can never interleave.
    let existing_basic_pay_cents: Option<i64> = sqlx::query_scalar(
        "SELECT basic_pay FROM compensation_terms
         WHERE employment_id = $1 AND effective_from = $2
         FOR UPDATE",
    )
    .bind(employment_id.as_str())
    .bind(current_effective_from)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(existing_basic_pay_cents) = existing_basic_pay_cents else {
        return Err(PayrollAppError::NoCompensationTermsRowAt {
            employment_id: employment_id.clone(),
            effective_from: current_effective_from,
        });
    };
    let old_basic_pay = Money::from_cents(existing_basic_pay_cents).expect(
        "compensation_terms.basic_pay CHECK: the column holds no negative amount, so it is a Money",
    );

    // The same "next row's effective_from" derivation `get_employment_snapshot`
    // makes: the row being corrected is in force until whichever later row
    // already exists, or open-ended.
    let next_effective_from: Option<NaiveDate> = sqlx::query_scalar(
        "SELECT MIN(effective_from) FROM compensation_terms
         WHERE employment_id = $1 AND effective_from > $2",
    )
    .bind(employment_id.as_str())
    .bind(current_effective_from)
    .fetch_one(&mut *tx)
    .await?;

    let diverging_periods = live_finalized_periods_in_span(
        &mut tx,
        employment_id,
        current_effective_from,
        next_effective_from,
    )
    .await?;

    sqlx::query(
        "UPDATE compensation_terms SET effective_from = $3, basic_pay = $4
         WHERE employment_id = $1 AND effective_from = $2",
    )
    .bind(employment_id.as_str())
    .bind(current_effective_from)
    .bind(new_effective_from)
    .bind(new_basic_pay.cents())
    .execute(&mut *tx)
    .await?;

    write_action_log_entry(
        &mut tx,
        ActionLogEntry {
            employer_id: &employer_id,
            actor: corrected_by,
            action_type: ActionType::CompensationTermsCorrected,
            target_type: "employment",
            target_id: employment_id.as_str(),
            context: Some(serde_json::json!({
                "reason": reason,
                "before": {
                    "effective_from": current_effective_from,
                    "basic_pay_cents": old_basic_pay.cents(),
                },
                "after": {
                    "effective_from": new_effective_from,
                    "basic_pay_cents": new_basic_pay.cents(),
                },
                "diverging_live_finalized_periods": diverging_periods
                    .iter()
                    .map(|period| {
                        serde_json::json!({
                            "period_start": period.start(),
                            "period_end": period.end(),
                        })
                    })
                    .collect::<Vec<_>>(),
            })),
        },
    )
    .await?;

    tx.commit().await?;
    Ok(diverging_periods)
}
