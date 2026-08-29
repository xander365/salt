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
pub async fn record_compensation_terms(
    pool: &PgPool,
    employment_id: &EmploymentId,
    effective_from: NaiveDate,
    basic_pay: Money,
    created_by: &str,
) -> Result<(), PayrollAppError> {
    let schedule_row: Option<(String, Option<i16>)> = sqlx::query_as(
        "SELECT employer.period_end_day_kind, employer.period_end_day_value
         FROM employment
         JOIN employer ON employer.id = employment.employer_id
         WHERE employment.id = $1",
    )
    .bind(employment_id.as_str())
    .fetch_optional(pool)
    .await?;
    let (kind, value) =
        schedule_row.ok_or_else(|| PayrollAppError::EmploymentNotFound(employment_id.clone()))?;
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
    .execute(pool)
    .await?;

    Ok(())
}
