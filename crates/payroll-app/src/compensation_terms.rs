//! `RecordCompensationTerms` and `CorrectCompensationTerms` (§4.4, §6.5,
//! §12). Recording inserts a new row; correcting changes an existing one.
//!
//! Both are payroll acts once they touch a span that already holds live
//! finalized payroll: §6.5 asks the same three things of every write that
//! makes master data disagree with paid history, whichever shape the write
//! takes. So both compute the same divergence list, require the same
//! acknowledgement of it, and write the same `CompensationTermsCorrected`
//! entry. What separates them is only that recording asks none of it of an
//! insert that diverges from nothing — the ordinary act of stating a pay
//! rise ahead of payroll, which stays as cheap as it ever was.

use std::collections::BTreeSet;

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::database::SaltDatabase;
use crate::employer::lock_the_pay_schedule_governing;
use crate::error::PayrollAppError;
use crate::freeze::{
    diverging_periods_json, live_finalized_periods_in_span, require_acknowledgement_of,
};
use chrono::NaiveDate;
use payroll::{EmploymentId, Money, PayPeriod, validate_effective_from_is_a_period_start};

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
///
/// Returns every Live finalized `PayPeriod` this new row now diverges from:
/// the periods already finalized inside the span it takes over,
/// `[effective_from, next_effective_from)`, `next_effective_from` being
/// whichever later row already exists for this Employment, or open-ended
/// when none does. That is the same derivation
/// `declare_unsupported_deduction_status` makes, computed here inside this
/// call's own transaction and under its Employment lock, so the list names
/// what the insert is about to disagree with.
///
/// Divergence is **never a refusal**. An insert onto a live finalized span
/// is the second half of §6.5's split (see [`correct_compensation_terms`]),
/// the one repair the whole correction machinery exists to enable — so it is
/// warned about, named and acknowledged, never blocked. But it is a
/// correction wearing an insert's clothes, and it is treated as one:
///
/// * `acknowledged_diverging_periods` must name exactly the returned list,
///   or the call is refused with
///   [`PayrollAppError::MasterDataDivergenceNotAcknowledged`] carrying it and
///   nothing is written — the same guard, from the same helper, as the two
///   correction paths (§6.5 guard 2).
/// * `reason` must be non-empty, and a `CompensationTermsCorrected` entry
///   carries it, the after values, and that list (§6.5 guard 1). The
///   `"before"` is `null`: there was no row, and the absence is recorded as
///   an absence rather than as a fabricated value.
///
/// An insert that diverges from nothing asks for none of that. Pass `&[]`
/// and an empty `reason`: an Employer stating next quarter's rise is not
/// correcting anything, no acknowledgement is owed, and no ActionLog entry
/// is written. Demanding a sentence there is the ceremony §5.1 rejects by
/// name.
pub async fn record_compensation_terms(
    db: &SaltDatabase,
    employment_id: &EmploymentId,
    effective_from: NaiveDate,
    basic_pay: Money,
    acknowledged_diverging_periods: &[PayPeriod],
    reason: &str,
    created_by: &str,
) -> Result<Vec<PayPeriod>, PayrollAppError> {
    let mut tx = db.pool().begin().await?;

    // The Employer row is locked first, and `FOR SHARE` is what makes the
    // `effective_from` guard below hold: `change_pay_schedule` takes `FOR
    // UPDATE` on that row and reads this table looking for an
    // `effective_from` its new schedule would strand, so without the lock a
    // schedule change and this write neither conflict nor see each other,
    // and both commit — leaving a row claiming a rise took effect on a day
    // that is no longer the start of anything (INV-014).
    let Some((employer_id, schedule)) =
        lock_the_pay_schedule_governing(&mut tx, employment_id).await?
    else {
        return Err(PayrollAppError::EmploymentNotFound(employment_id.clone()));
    };

    // `FOR UPDATE` holds the row against a concurrent `void_employment`,
    // whose `UPDATE` needs the exclusive lock — without it, a void
    // committing between this read and the insert would leave a
    // CompensationTerms row recorded against a voided Employment.
    //
    // It is the exclusive lock rather than `FOR SHARE`, as in
    // `correct_compensation_terms`, because this call now derives a span
    // from its sibling rows: two inserts for one Employment would otherwise
    // not conflict, and each would compute a divergence list the other's
    // commit falsifies. It also conflicts with `finalize_payroll_run`'s `FOR
    // SHARE`, so the `live_finalized_payroll` rows the list is read from
    // cannot grow underneath it either.
    let is_void: Option<bool> =
        sqlx::query_scalar("SELECT is_void FROM employment WHERE id = $1 FOR UPDATE")
            .bind(employment_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
    let is_void =
        is_void.ok_or_else(|| PayrollAppError::EmploymentNotFound(employment_id.clone()))?;
    if is_void {
        return Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()));
    }

    validate_effective_from_is_a_period_start(schedule, effective_from)?;

    // The span this insert takes over, derived exactly as
    // `declare_unsupported_deduction_status` derives its own: up to whichever
    // later row already exists for this Employment, or open-ended when none
    // does. Read before the INSERT below, so it names the span as it will
    // stand once the row is in.
    let next_effective_from: Option<NaiveDate> = sqlx::query_scalar(
        "SELECT MIN(effective_from) FROM compensation_terms
         WHERE employment_id = $1 AND effective_from > $2",
    )
    .bind(employment_id.as_str())
    .bind(effective_from)
    .fetch_one(&mut *tx)
    .await?;

    let diverging_periods =
        live_finalized_periods_in_span(&mut tx, employment_id, effective_from, next_effective_from)
            .await?;

    // Acknowledgement is checked before the reason, so the caller that asks
    // with nothing acknowledged and nothing to say is told *what* diverges
    // rather than being sent away for a sentence it has no reason to write
    // yet. An insert that diverges from nothing passes both checks silently.
    require_acknowledgement_of(
        employment_id,
        &diverging_periods,
        acknowledged_diverging_periods,
    )?;

    let diverges = !diverging_periods.is_empty();
    if diverges && reason.trim().is_empty() {
        return Err(PayrollAppError::CompensationTermsCorrectionReasonCannotBeEmpty);
    }

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

    // Only a diverging insert is a correction. One that diverges from
    // nothing is the ordinary act of stating a pay rise ahead of payroll,
    // and logging it as a correction would say something untrue about it.
    if diverges {
        write_action_log_entry(
            &mut tx,
            ActionLogEntry {
                employer_id: &employer_id,
                actor: created_by,
                action_type: ActionType::CompensationTermsCorrected,
                target_type: "employment",
                target_id: employment_id.as_str(),
                context: Some(serde_json::json!({
                    "reason": reason,
                    // No row governed `effective_from` under this key before
                    // now. The absence is recorded as an absence; a "before"
                    // carrying the amount some earlier row happened to state
                    // would name a value this insert never replaced.
                    "before": serde_json::Value::Null,
                    "after": {
                        "effective_from": effective_from,
                        "basic_pay_cents": basic_pay.cents(),
                    },
                    "diverging_live_finalized_periods": diverging_periods_json(&diverging_periods),
                })),
            },
        )
        .await?;
    }

    tx.commit().await?;

    Ok(diverging_periods)
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
/// Returns every Live finalized `PayPeriod` whose governing
/// `CompensationTerms` can differ after this correction: the row's old span
/// and its new span. Computed before the row is updated, so the list names
/// exactly what this correction is about to disagree with, and it reads
/// `finalized_payroll` and `live_finalized_payroll` without writing either
/// (§6.5 guard 3).
///
/// That list must also be **acknowledged**: `acknowledged_diverging_periods`
/// has to name exactly the same periods, or the call is refused with
/// [`PayrollAppError::MasterDataDivergenceNotAcknowledged`] carrying the
/// list, and nothing is written (§6.5 guard 2). Divergence is still a
/// warning and never a refusal of the correction itself — acknowledging it
/// carries the identical correction through — but a caller cannot log an
/// acknowledgement of a list it was never shown. A correction that diverges
/// from nothing acknowledges nothing: pass `&[]`.
///
/// Splitting a row (§6.5's March/April/May case) is two separate calls, in
/// this order: this call first moves the existing row's `effective_from`
/// forward, then [`record_compensation_terms`] inserts the new earlier row.
/// The move frees the original `(employment_id, effective_from)` key for the
/// insert, while its divergence list still names the original wider span.
// Eight facts, each named at the call site, and no two of them belong
// together in a struct: a parameter object here would only be this list with
// one more name in front of it.
#[allow(clippy::too_many_arguments)]
pub async fn correct_compensation_terms(
    db: &SaltDatabase,
    employment_id: &EmploymentId,
    current_effective_from: NaiveDate,
    new_effective_from: NaiveDate,
    new_basic_pay: Money,
    acknowledged_diverging_periods: &[PayPeriod],
    reason: &str,
    corrected_by: &str,
) -> Result<Vec<PayPeriod>, PayrollAppError> {
    if reason.trim().is_empty() {
        return Err(PayrollAppError::CompensationTermsCorrectionReasonCannotBeEmpty);
    }

    let mut tx = db.pool().begin().await?;

    // Same lock, same reason as `record_compensation_terms`: it is what
    // makes the INV-014 check on `new_effective_from` below hold against a
    // concurrent `change_pay_schedule`.
    let Some((employer_id, schedule)) =
        lock_the_pay_schedule_governing(&mut tx, employment_id).await?
    else {
        return Err(PayrollAppError::EmploymentNotFound(employment_id.clone()));
    };

    // This exclusive Employment lock serializes every effective-dated
    // master-data write for the Employment. A sibling insert would otherwise
    // be able to change the row's span after the divergence list is read but
    // before this correction commits.
    let is_void: Option<bool> =
        sqlx::query_scalar("SELECT is_void FROM employment WHERE id = $1 FOR UPDATE")
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

    // A correction can move a row across sibling boundaries. Name the union
    // of its old and new spans so every finalized period whose governing
    // terms can change reaches both the caller and ActionLog.
    let old_next_effective_from: Option<NaiveDate> = sqlx::query_scalar(
        "SELECT MIN(effective_from) FROM compensation_terms
         WHERE employment_id = $1 AND effective_from > $2",
    )
    .bind(employment_id.as_str())
    .bind(current_effective_from)
    .fetch_one(&mut *tx)
    .await?;

    let mut diverging_periods: BTreeSet<PayPeriod> = live_finalized_periods_in_span(
        &mut tx,
        employment_id,
        current_effective_from,
        old_next_effective_from,
    )
    .await?
    .into_iter()
    .collect();

    if new_effective_from != current_effective_from {
        // Exclude the old row because the UPDATE below removes it. This gives
        // the first row that will bound the corrected row after the move.
        let new_next_effective_from: Option<NaiveDate> = sqlx::query_scalar(
            "SELECT MIN(effective_from) FROM compensation_terms
             WHERE employment_id = $1
               AND effective_from > $2
               AND effective_from <> $3",
        )
        .bind(employment_id.as_str())
        .bind(new_effective_from)
        .bind(current_effective_from)
        .fetch_one(&mut *tx)
        .await?;

        diverging_periods.extend(
            live_finalized_periods_in_span(
                &mut tx,
                employment_id,
                new_effective_from,
                new_next_effective_from,
            )
            .await?,
        );
    }

    let diverging_periods: Vec<PayPeriod> = diverging_periods.into_iter().collect();

    // Checked against the list this transaction has just computed, under the
    // Employment lock, so what is acknowledged is what is about to be true —
    // and refused before the UPDATE, so a refusal leaves the row untouched.
    require_acknowledgement_of(
        employment_id,
        &diverging_periods,
        acknowledged_diverging_periods,
    )?;

    // A move onto a date this Employment already has a row at collides with
    // `UNIQUE (employment_id, effective_from)`. Named here as the domain
    // refusal it is, rather than handed back as a database error.
    if new_effective_from != current_effective_from {
        let taken: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM compensation_terms
                WHERE employment_id = $1 AND effective_from = $2
            )",
        )
        .bind(employment_id.as_str())
        .bind(new_effective_from)
        .fetch_one(&mut *tx)
        .await?;
        if taken {
            return Err(PayrollAppError::CompensationTermsAlreadyExistAt {
                employment_id: employment_id.clone(),
                effective_from: new_effective_from,
            });
        }
    }

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
                // This entry existing is the record of the acknowledgement:
                // the call is refused above unless the caller named exactly
                // this list, so §6.5's "nowhere else" needs no second table.
                "diverging_live_finalized_periods": diverging_periods_json(&diverging_periods),
            })),
        },
    )
    .await?;

    tx.commit().await?;
    Ok(diverging_periods)
}
