//! `RecordCompensationTerms` (§4.4, §12). Recording only: correcting an
//! existing row is a separate, later use case, because §6.5 makes a
//! correction a payroll act that gathers a reason and computes a divergence
//! list against live finalized periods — not something a bare record call
//! can do.

use chrono::NaiveDate;
use payroll::{EmploymentId, Money, validate_compensation_terms_effective_from};
use sqlx::PgPool;

use crate::employer::pay_schedule_from_columns;
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

    // `FOR SHARE OF employment` holds the row against a concurrent
    // `void_employment`, whose `UPDATE` needs the exclusive lock. Without
    // it, a void committing between this read and the insert would leave a
    // CompensationTerms row recorded against a voided Employment.
    let schedule_row: Option<(String, Option<i16>, bool)> = sqlx::query_as(
        "SELECT employer.period_end_day_kind,
                employer.period_end_day_value,
                employment.is_void
         FROM employment
         JOIN employer ON employer.id = employment.employer_id
         WHERE employment.id = $1
         FOR SHARE OF employment",
    )
    .bind(employment_id.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    let (kind, value, is_void) =
        schedule_row.ok_or_else(|| PayrollAppError::EmploymentNotFound(employment_id.clone()))?;
    if is_void {
        return Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()));
    }
    let schedule = pay_schedule_from_columns(&kind, value);

    validate_compensation_terms_effective_from(schedule, effective_from)?;

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
