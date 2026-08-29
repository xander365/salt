//! `CreateEmployer` (§4.2, §12). The `employer` table carries one
//! `period_end_day_kind`/`period_end_day_value` pair per row, so "exactly
//! one `PaySchedule`" is structural — this use case needs no extra guard
//! for it.

use chrono::NaiveDate;
use payroll::{DayOfMonth, EmployerId, PaySchedule, PeriodEndDay, TaxYear};
use sqlx::PgPool;

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::error::PayrollAppError;
use crate::freeze::employer_has_a_finalization_in_or_after;
use crate::ids::new_id;

/// Records a new Employer with the given `PaySchedule`. Changing it later is
/// [`change_pay_schedule`].
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

/// Changes an Employer's `PaySchedule` (§4.2), refused once any Employment
/// of theirs has a `FinalizedPayroll` in `current_tax_year` **or any later
/// TaxYear** (ADR-0013, issue #33) — the guard that keeps a TaxYear at
/// exactly twelve periods and cumulative PAYE sound. Deliberately not
/// effective-dated: the `employer` table carries exactly one `PaySchedule`,
/// per its own docstring above, so this simply overwrites it, and the
/// refusal is what confines a change to a TaxYear that has finalized nothing
/// yet — the next one, in practice, once the current TaxYear has any
/// finalized payroll. History stays safe regardless: a `FinalizedPayroll`
/// freezes the schedule it actually used, so an Employer's schedule changing
/// under it later changes nothing about what already happened.
///
/// `current_tax_year` is the caller's own account of which TaxYear this
/// change is being made in, for the same reason every other use case here
/// takes its dates as parameters rather than reading the wall clock. It is a
/// claim, not a fact, so this use case never rests the twelve-period
/// invariant on it alone:
///
/// - Naming a TaxYear *earlier* than the finalized one is caught here: the
///   check reads "in `current_tax_year` or later", so a 2026 finalization
///   refuses a change claimed for 2025 as well as one claimed for 2026.
/// - Naming a *later* TaxYear cannot be disproved without a clock, so it is
///   caught where the harm would actually land instead:
///   `create_ordinary_payroll_run` refuses a run in a TaxYear whose already
///   finalized periods the current schedule does not generate. A schedule
///   moved mid-year therefore buys nothing — no further period of that
///   TaxYear can be run under it.
///
/// An unfinalized `PayrollRun` in `current_tax_year` also refuses. That run's
/// period was built from the schedule in force when it was created, and
/// finalization re-derives everything from the *current* schedule (§5.1), so
/// letting the change through would trade a clear refusal now for a confusing
/// `FinalizationInputMismatch` later.
pub async fn change_pay_schedule(
    pool: &PgPool,
    employer_id: &EmployerId,
    new_schedule: PaySchedule,
    current_tax_year: TaxYear,
    changed_by: &str,
) -> Result<(), PayrollAppError> {
    let mut tx = pool.begin().await?;

    // `FOR UPDATE` is what holds the freeze check below against a concurrent
    // *reader* of this schedule: `pay_schedule_for_employer` takes `FOR
    // SHARE` on this row, and `create_ordinary_payroll_run` takes its own
    // `FOR UPDATE`, so a finalization or a run creation can neither commit
    // between this check and the write below nor read a schedule this
    // transaction is about to replace. It also serialises two concurrent
    // changes against each other.
    let exists: Option<(i32,)> = sqlx::query_as("SELECT 1 FROM employer WHERE id = $1 FOR UPDATE")
        .bind(employer_id.as_str())
        .fetch_optional(&mut *tx)
        .await?;
    if exists.is_none() {
        return Err(PayrollAppError::EmployerNotFound(employer_id.clone()));
    }

    if employer_has_a_finalization_in_or_after(&mut tx, employer_id, current_tax_year).await? {
        return Err(PayrollAppError::PayScheduleFrozenByFinalization {
            employer_id: employer_id.clone(),
            tax_year: current_tax_year,
        });
    }

    // The TaxYear of an unfinalized run is decided in Rust, by the same
    // `TaxYear::for_period_end` every other reader of a period end uses,
    // rather than as a date range in SQL — one statement of where a TaxYear
    // begins, not two.
    let open_run_period_ends: Vec<NaiveDate> = sqlx::query_scalar(
        "SELECT period_end FROM payroll_run
         WHERE employer_id = $1 AND status <> 'finalized'",
    )
    .bind(employer_id.as_str())
    .fetch_all(&mut *tx)
    .await?;
    if let Some(period_end) = open_run_period_ends
        .into_iter()
        .find(|period_end| TaxYear::for_period_end(*period_end) == current_tax_year)
    {
        return Err(PayrollAppError::PayScheduleChangeBlockedByAnOpenRun {
            employer_id: employer_id.clone(),
            period_end,
        });
    }

    let (kind, value) = period_end_day_columns(new_schedule.period_end_day());
    sqlx::query(
        "UPDATE employer SET period_end_day_kind = $2, period_end_day_value = $3 WHERE id = $1",
    )
    .bind(employer_id.as_str())
    .bind(kind)
    .bind(value)
    .execute(&mut *tx)
    .await?;

    write_action_log_entry(
        &mut tx,
        ActionLogEntry {
            employer_id,
            actor: changed_by,
            action_type: ActionType::PayScheduleChanged,
            target_type: "employer",
            target_id: employer_id.as_str(),
            context: None,
        },
    )
    .await?;

    tx.commit().await?;
    Ok(())
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
///
/// `FOR SHARE` is what makes the read hold: [`change_pay_schedule`] takes
/// `FOR UPDATE` on the same row, so a schedule change can neither commit
/// between this read and the caller's own writes, nor slip its freeze check
/// past a calculation or finalization already under way. The two locks
/// conflict, so whichever transaction arrives second waits and then sees the
/// other's committed result rather than a stale one.
pub(crate) async fn pay_schedule_for_employer(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    employer_id: &EmployerId,
) -> Result<PaySchedule, PayrollAppError> {
    let (kind, value): (String, Option<i16>) = sqlx::query_as(
        "SELECT period_end_day_kind, period_end_day_value FROM employer
         WHERE id = $1 FOR SHARE",
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
