//! `RecordCompensationTerms` (§4.4, §12). Recording only: correcting an
//! existing row is a separate, later use case, because §6.5 makes a
//! correction a payroll act that gathers a reason and computes a divergence
//! list against live finalized periods — not something a bare record call
//! can do.

use chrono::NaiveDate;
use payroll::{EmploymentId, Money, validate_effective_from_is_a_period_start};
use sqlx::PgPool;

use crate::employer::lock_the_pay_schedule_governing;
use crate::error::PayrollAppError;

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
