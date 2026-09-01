//! `CreateEmployer` (§4.2, §12). The `employer` table carries one
//! `period_end_day_kind`/`period_end_day_value` pair per row, so "exactly
//! one `PaySchedule`" is structural — this use case needs no extra guard
//! for it.

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::database::SaltDatabase;
use crate::error::{PayrollAppError, ScheduleBoundedFact};
use crate::freeze::employer_has_a_finalization_in_or_after;
use crate::ids::new_id;
use chrono::NaiveDate;
use payroll::{
    DayOfMonth, EmployerId, EmploymentId, PaySchedule, PayrollError, PeriodEndDay, TaxYear,
    validate_effective_from_is_a_period_start,
};

/// Records a new Employer with the given `PaySchedule`. Changing it later is
/// [`change_pay_schedule`].
pub async fn create_employer(
    db: &SaltDatabase,
    pay_schedule: PaySchedule,
    created_by: &str,
) -> Result<EmployerId, PayrollAppError> {
    let pool = db.pool();
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
/// An unfinalized `PayrollRun` also refuses, **whatever TaxYear its period
/// falls in**. That run's period was cut by the schedule in force when it was
/// created, and finalization re-derives everything from the *current* one
/// (§5.1), so a change under it leaves a run that can neither be finalized
/// nor re-created — `create_ordinary_payroll_run` refuses the old period too.
/// `current_tax_year` deliberately does not narrow this check: it is the
/// caller's claim, and a run that exists is a fact, so resting a stranding on
/// the claim would be the one place this use case trusts it.
///
/// A stored boundary the new schedule would strand also refuses — see
/// `validate_the_new_schedule_strands_no_stored_boundary` below. Refusing
/// nothing else is deliberate: an Employer who has finalized nothing, has no
/// run open, and recorded no boundary in `current_tax_year` is exactly the one
/// who set the schedule up wrong on Monday and wants it right on Tuesday.
pub async fn change_pay_schedule(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    new_schedule: PaySchedule,
    current_tax_year: TaxYear,
    changed_by: &str,
) -> Result<(), PayrollAppError> {
    let mut tx = db.pool().begin().await?;

    // `FOR UPDATE` is what holds the freeze check below against a concurrent
    // *reader* of this schedule: `pay_schedule_for_employer` takes `FOR
    // SHARE` on this row, and `create_ordinary_payroll_run` takes its own
    // `FOR UPDATE`, so a finalization or a run creation can neither commit
    // between this check and the write below nor read a schedule this
    // transaction is about to replace. It also serialises two concurrent
    // changes against each other.
    let current: Option<(String, Option<i16>)> = sqlx::query_as(
        "SELECT period_end_day_kind, period_end_day_value FROM employer
         WHERE id = $1 FOR UPDATE",
    )
    .bind(employer_id.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    let Some((kind, value)) = current else {
        return Err(PayrollAppError::EmployerNotFound(employer_id.clone()));
    };
    let current_schedule = pay_schedule_from_columns(&kind, value);

    if employer_has_a_finalization_in_or_after(&mut tx, employer_id, current_tax_year).await? {
        return Err(PayrollAppError::PayScheduleFrozenByFinalization {
            employer_id: employer_id.clone(),
            tax_year: current_tax_year,
        });
    }

    // Any unfinalized run at all, in any TaxYear — see the docstring. The
    // earliest is named because it is the one an Employer resolves first,
    // and because naming a stable one keeps the refusal reproducible.
    let open_run_period_end: Option<NaiveDate> = sqlx::query_scalar(
        "SELECT period_end FROM payroll_run
         WHERE employer_id = $1 AND status <> 'finalized'
         ORDER BY period_end
         LIMIT 1",
    )
    .bind(employer_id.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(period_end) = open_run_period_end {
        return Err(PayrollAppError::PayScheduleChangeBlockedByAnOpenRun {
            employer_id: employer_id.clone(),
            period_end,
        });
    }

    validate_the_new_schedule_strands_no_stored_boundary(
        &mut tx,
        employer_id,
        current_schedule,
        new_schedule,
        current_tax_year,
    )
    .await?;

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

/// Refuses a `PaySchedule` change that would leave a boundary already stored
/// in `current_tax_year` or later on a date `new_schedule` does not generate
/// (§4.2, §4.4, §4.5 guard 1, §4.5c).
///
/// The `employer` table holds one `PaySchedule` and no history, so a change
/// is retroactive for everything that is not already frozen into a
/// `PayrollInput`. The finalization refusal above protects the periods that
/// *have* been paid; this one protects the dated facts standing ready for the
/// periods that have not. Without it a `SaltCoverageStart` recorded on the
/// 25th survives a move to calendar months as a boundary falling mid-period —
/// the single thing user story 11 exists to prevent — and a
/// `CompensationTerms` row goes on claiming a rise took effect on a day that
/// is no longer the start of anything (INV-014).
///
/// Only boundaries from `current_tax_year` onward are checked. An earlier
/// one describes a period already paid, whose `FinalizedPayroll` froze the
/// schedule that cut it (§4.2), so nothing re-reads it against today's
/// schedule; checking those too would make the "change it from the next
/// TaxYear" path the design record promises unreachable for any Employer who
/// has ever run a payroll.
///
/// `opening_balance` is filtered by its own `tax_year` column. The two
/// effective-dated tables have no such column — their `effective_from` is a
/// `PayPeriod` **start**, and a TaxYear's first period may start in the
/// February before it (ADR-0005) — so they are filtered by the first period
/// start `current_schedule` itself generates in `current_tax_year`, which is
/// the same arithmetic read from the same place rather than a date range
/// guessed at in SQL.
///
/// A void Employment's facts are excluded: it reaches no payroll (§4.3), so
/// no boundary of its own can strand anything.
async fn validate_the_new_schedule_strands_no_stored_boundary(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    employer_id: &EmployerId,
    current_schedule: PaySchedule,
    new_schedule: PaySchedule,
    current_tax_year: TaxYear,
) -> Result<(), PayrollAppError> {
    let salt_coverage_starts: Vec<NaiveDate> = sqlx::query_scalar(
        "SELECT opening_balance.first_salt_period_end
         FROM opening_balance
         JOIN employment ON employment.id = opening_balance.employment_id
         WHERE employment.employer_id = $1
           AND NOT employment.is_void
           AND opening_balance.tax_year >= $2
         ORDER BY opening_balance.first_salt_period_end",
    )
    .bind(employer_id.as_str())
    .bind(current_tax_year.starting_year())
    .fetch_all(&mut **tx)
    .await?;
    refuse_any_stranded_boundary(
        employer_id,
        ScheduleBoundedFact::SaltCoverageStart,
        salt_coverage_starts,
        |boundary| generates_the_period_end(new_schedule, boundary),
    )?;

    let tax_year_first_period_start = first_period_start_of(current_schedule, current_tax_year)?;

    // The two effective-dated tables are read by two statements written
    // out, not by one query built around a table name: this crate
    // interpolates no SQL, and the pair differ only in what they read from.
    let compensation_effective_froms: Vec<NaiveDate> = sqlx::query_scalar(
        "SELECT compensation_terms.effective_from
         FROM compensation_terms
         JOIN employment ON employment.id = compensation_terms.employment_id
         WHERE employment.employer_id = $1
           AND NOT employment.is_void
           AND compensation_terms.effective_from >= $2
         ORDER BY compensation_terms.effective_from",
    )
    .bind(employer_id.as_str())
    .bind(tax_year_first_period_start)
    .fetch_all(&mut **tx)
    .await?;
    refuse_any_stranded_boundary(
        employer_id,
        ScheduleBoundedFact::CompensationTermsEffectiveFrom,
        compensation_effective_froms,
        |boundary| validate_effective_from_is_a_period_start(new_schedule, boundary).is_ok(),
    )?;

    let declaration_effective_froms: Vec<NaiveDate> = sqlx::query_scalar(
        "SELECT unsupported_deduction_declaration.effective_from
         FROM unsupported_deduction_declaration
         JOIN employment
           ON employment.id = unsupported_deduction_declaration.employment_id
         WHERE employment.employer_id = $1
           AND NOT employment.is_void
           AND unsupported_deduction_declaration.effective_from >= $2
         ORDER BY unsupported_deduction_declaration.effective_from",
    )
    .bind(employer_id.as_str())
    .bind(tax_year_first_period_start)
    .fetch_all(&mut **tx)
    .await?;
    refuse_any_stranded_boundary(
        employer_id,
        ScheduleBoundedFact::UnsupportedDeductionEffectiveFrom,
        declaration_effective_froms,
        |boundary| validate_effective_from_is_a_period_start(new_schedule, boundary).is_ok(),
    )?;

    Ok(())
}

/// Refuses the first of `boundaries` the new schedule does not place, naming
/// it as `fact`. The three queries above stay written out — this crate builds
/// no SQL by string interpolation — but what they *mean* is one thing said
/// once: find a boundary the schedule no longer places, and refuse it.
fn refuse_any_stranded_boundary(
    employer_id: &EmployerId,
    fact: ScheduleBoundedFact,
    boundaries: Vec<NaiveDate>,
    is_placed_by_the_new_schedule: impl Fn(NaiveDate) -> bool,
) -> Result<(), PayrollAppError> {
    match boundaries
        .into_iter()
        .find(|boundary| !is_placed_by_the_new_schedule(*boundary))
    {
        Some(boundary) => Err(
            PayrollAppError::PayScheduleChangeWouldStrandAStoredBoundary {
                employer_id: employer_id.clone(),
                fact,
                boundary,
            },
        ),
        None => Ok(()),
    }
}

/// Whether `schedule` generates a `PayPeriod` ending exactly on `date` — the
/// same question §4.5 guard 1 asks of a `SaltCoverageStart`, and the one
/// `create_ordinary_payroll_run` asks of an already finalized period end.
pub(crate) fn generates_the_period_end(schedule: PaySchedule, date: NaiveDate) -> bool {
    schedule
        .period_containing(date)
        .is_some_and(|period| period.end() == date)
}

/// The start date of `tax_year`'s own first `PayPeriod` under `schedule`.
///
/// Every TaxYear's first period ends in its own March (ADR-0005), so 1 March
/// of the starting year always falls inside it; the start may be in the
/// February before. Refused rather than panicked on at the very edge of the
/// representable calendar, exactly as every other date derivation in this
/// crate is — and the two edges are refused separately, so the refusal always
/// names something the caller actually supplied: a TaxYear with no 1 March,
/// or a schedule that cannot reach back from one.
fn first_period_start_of(
    schedule: PaySchedule,
    tax_year: TaxYear,
) -> Result<NaiveDate, PayrollAppError> {
    let march_first = NaiveDate::from_ymd_opt(tax_year.starting_year(), 3, 1)
        .ok_or(PayrollAppError::TaxYearOutsideRepresentableCalendar { tax_year })?;
    schedule
        .period_containing(march_first)
        .map(|period| period.start())
        .ok_or(PayrollError::PayScheduleOutsideRepresentableCalendar { date: march_first })
        .map_err(PayrollAppError::from)
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

/// Locks the Employer of `employment_id` with `FOR SHARE` and hands back
/// their id and current `PaySchedule`, or `None` when no such Employment
/// exists.
///
/// Every use case that stores a **schedule-bounded date** — a
/// `SaltCoverageStart`, a `CompensationTerms` `effective_from`, an
/// `UnsupportedDeductionStatus` `effective_from` — must take this lock
/// before it validates that date, because
/// [`change_pay_schedule`]'s stranded-boundary guard reads those three
/// tables under its own `FOR UPDATE` on this row. Without the lock the two
/// transactions do not conflict: the writer validates against the schedule
/// its snapshot shows, the changer's guard runs before the writer's row is
/// visible, and both commit — leaving exactly the stranded boundary that
/// guard exists to refuse.
///
/// The Employer row is locked **before** the Employment row, never after,
/// and this separate statement is what fixes that order. Every use case that
/// holds both takes them the same way round — `create_ordinary_payroll_run`
/// locks the `employer` row and then touches `employment` through its
/// membership insert's foreign key, and `finalize_payroll_run` reads the
/// schedule before it locks its members — so no two of them can deadlock.
pub(crate) async fn lock_the_pay_schedule_governing(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    employment_id: &EmploymentId,
) -> Result<Option<(EmployerId, PaySchedule)>, PayrollAppError> {
    let row: Option<(String, String, Option<i16>)> = sqlx::query_as(
        "SELECT id, period_end_day_kind, period_end_day_value FROM employer
         WHERE id = (SELECT employer_id FROM employment WHERE id = $1)
         FOR SHARE",
    )
    .bind(employment_id.as_str())
    .fetch_optional(&mut **tx)
    .await?;

    Ok(row.map(|(employer_id, kind, value)| {
        (
            EmployerId::new(employer_id),
            pay_schedule_from_columns(&kind, value),
        )
    }))
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
