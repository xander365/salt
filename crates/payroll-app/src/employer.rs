//! `CreateEmployer` (§4.2, §12). The `employer` table carries one
//! `period_end_day_kind`/`period_end_day_value` pair per row, so "exactly
//! one `PaySchedule`" is structural — this use case needs no extra guard
//! for it.

use payroll::{DayOfMonth, EmployerId, PaySchedule, PeriodEndDay};
use sqlx::PgPool;

use crate::error::PayrollAppError;
use crate::ids::new_id;

/// Records a new Employer with the given `PaySchedule`. A later change to
/// that schedule is a separate use case (§4.2: legal only at a TaxYear
/// boundary, which is domain reasoning this ticket does not implement).
pub async fn create_employer(
    pool: &PgPool,
    pay_schedule: PaySchedule,
    created_by: &str,
) -> Result<EmployerId, PayrollAppError> {
    let id = EmployerId::new(new_id());
    let (kind, value) = period_end_day_columns(pay_schedule.period_end_day());

    sqlx::query(
        "INSERT INTO employer (id, period_end_day_kind, period_end_day_value, created_by)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(id.as_str())
    .bind(kind)
    .bind(value)
    .bind(created_by)
    .execute(pool)
    .await?;

    Ok(id)
}

fn period_end_day_columns(period_end_day: PeriodEndDay) -> (&'static str, Option<i16>) {
    match period_end_day {
        PeriodEndDay::Day(day) => ("day", Some(i16::from(day.get()))),
        PeriodEndDay::LastDayOfMonth => ("last_day_of_month", None),
    }
}

/// The inverse of [`period_end_day_columns`], for a use case that must
/// re-derive the Employer's `PaySchedule` to check a date against it (e.g.
/// `record_compensation_terms`'s INV-014 check). Panics rather than
/// returning a `Result`: the `employer` table's own CHECK constraint
/// already guarantees `kind` and `value` agree, so disagreement here would
/// mean the schema itself no longer matches this code, not a fact about the
/// Employer being read.
pub(crate) fn pay_schedule_from_columns(kind: &str, value: Option<i16>) -> PaySchedule {
    let period_end_day = match kind {
        "day" => {
            let value = value.expect("employer CHECK: period_end_day_kind = 'day' carries a value");
            let day = u8::try_from(value)
                .ok()
                .and_then(|day| DayOfMonth::new(day).ok())
                .expect("employer CHECK: period_end_day_value is between 1 and 28");
            PeriodEndDay::Day(day)
        }
        "last_day_of_month" => PeriodEndDay::LastDayOfMonth,
        other => panic!(
            "employer CHECK: period_end_day_kind is 'day' or 'last_day_of_month', found {other:?}"
        ),
    };
    PaySchedule::new(period_end_day)
}

/// The Employer's own `PaySchedule`, read on the caller's transaction —
/// the one read every use case that must calculate against an Employer's
/// periods starts with. One function rather than the same two-column
/// `query_as` written out beside every caller of
/// [`pay_schedule_from_columns`].
pub(crate) async fn pay_schedule_for_employer(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    employer_id: &EmployerId,
) -> Result<PaySchedule, PayrollAppError> {
    let (kind, value): (String, Option<i16>) = sqlx::query_as(
        "SELECT period_end_day_kind, period_end_day_value FROM employer WHERE id = $1",
    )
    .bind(employer_id.as_str())
    .fetch_one(&mut **tx)
    .await?;
    Ok(pay_schedule_from_columns(&kind, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_end_day_columns_round_trips_a_fixed_day() {
        let schedule = PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap()));
        let (kind, value) = period_end_day_columns(schedule.period_end_day());
        assert_eq!(
            pay_schedule_from_columns(kind, value).period_end_day(),
            schedule.period_end_day()
        );
    }

    #[test]
    fn period_end_day_columns_round_trips_the_last_day_of_month() {
        let schedule = PaySchedule::new(PeriodEndDay::LastDayOfMonth);
        let (kind, value) = period_end_day_columns(schedule.period_end_day());
        assert_eq!(
            pay_schedule_from_columns(kind, value).period_end_day(),
            schedule.period_end_day()
        );
    }
}
