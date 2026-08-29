//! `CreateOrdinaryPayrollRun`, `RemoveEmploymentFromRun` and
//! `SetRunEarnings` — the Ordinary half of §4.6-§4.8 and §4.5d, §12.
//! `CreateCorrectionRun`, calculation and finalization are separate, later
//! use cases: Ordinary and Correction membership are opposites (§4.8,
//! ADR-0015), so a single entry point taking a `kind` would branch on its
//! first line and share nothing after it.

use chrono::NaiveDate;
use payroll::{Earning, EmployerId, EmploymentId, PayPeriod, PaySchedule, PayrollError};
use sqlx::PgPool;

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::employer::pay_schedule_from_columns;
use crate::error::PayrollAppError;

/// `payroll-app`'s own id (§4.1): a native UUID, unlike the pure crate's
/// opaque `TEXT`-backed ids. Held here as its canonical text form rather
/// than a `uuid::Uuid` so every query can bind and read it exactly like
/// `EmploymentId`, with an explicit `::uuid`/`::text` cast in the SQL where
/// the column type must be pinned down.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PayrollRunId(String);

impl PayrollRunId {
    fn new(id: impl Into<String>) -> Self {
        PayrollRunId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PayrollRunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Creates a Draft Ordinary `PayrollRun` for `employer_id` and `period`, and
/// writes a membership row for every Employment overlapping `period` —
/// silent omission is the dangerous failure, so auto-inclusion makes
/// omission a deliberate, reasoned, logged act instead (§4.8). A voided
/// Employment is never proposed: the compound foreign key from migration
/// 0018 also refuses one outright if this changed.
///
/// `pay_date` is stored on the run and is never a calculation input — it
/// does not reach `PayrollInput` (§4.6).
///
/// Overlap is decided in Rust against the Employment's own `start_date` and
/// `end_date`, never as a SQL range operator, so the same rule
/// `EmploymentSnapshot::employed_days_within` uses elsewhere in the domain
/// governs membership too.
///
/// `period` must be one the Employer's own `PaySchedule` generates, the
/// same demand INV-014 makes of a `CompensationTerms` start date and §4.5
/// guard 1 makes of a `SaltCoverageStart`. Everything downstream reads a
/// run's period as one of the schedule's twelve: sequencing walks back to
/// "the immediately preceding PayPeriod" (§8), the Ordinary uniqueness
/// index keys on the period end alone (§4.6), and `calculate` resolves the
/// `CompensationTerms` in force from the period's own boundaries. An
/// invented period has no predecessor to walk back to, so it is refused
/// here rather than left to fail confusingly at finalization.
///
/// A second Ordinary run for the same Employer and PayPeriod is refused by
/// the unique index `one_ordinary_payroll_run_per_employer_and_period`
/// (migration 0007) — a PostgreSQL refusal, not a domain one, the same as
/// every other uniqueness violation in this crate.
pub async fn create_ordinary_payroll_run(
    pool: &PgPool,
    employer_id: &EmployerId,
    period: PayPeriod,
    pay_date: NaiveDate,
    created_by: &str,
) -> Result<PayrollRunId, PayrollAppError> {
    let mut tx = pool.begin().await?;

    // One read does two jobs: it proves the Employer exists and yields the
    // `PaySchedule` `period` is checked against, and it locks the Employer.
    //
    // Creating an Employment takes PostgreSQL's KEY SHARE lock on its
    // Employer through the foreign key. Taking UPDATE here makes that
    // creation serialize with this membership snapshot: an Employment that
    // commits before the run does is either visible below, or waited until the
    // fully-populated run commits. Without this lock, one could commit after
    // the employment SELECT below and be silently absent from the only
    // Ordinary run.
    let schedule_row: Option<(String, Option<i16>)> = sqlx::query_as(
        "SELECT period_end_day_kind, period_end_day_value FROM employer
         WHERE id = $1 FOR UPDATE",
    )
    .bind(employer_id.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    let Some((kind, value)) = schedule_row else {
        return Err(PayrollAppError::EmployerNotFound(employer_id.clone()));
    };

    validate_period_is_one_the_schedule_generates(pay_schedule_from_columns(&kind, value), period)?;

    let id: String = sqlx::query_scalar(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status, created_by)
         VALUES ($1, $2, $3, $4, 'ordinary', 'draft', $5)
         RETURNING id::text",
    )
    .bind(employer_id.as_str())
    .bind(period.start())
    .bind(period.end())
    .bind(pay_date)
    .bind(created_by)
    .fetch_one(&mut *tx)
    .await?;
    let run_id = PayrollRunId::new(id);

    // Every active Employment for this Employer is a membership candidate;
    // the overlap decision below is what actually admits one.
    let employments: Vec<(String, NaiveDate, Option<NaiveDate>)> = sqlx::query_as(
        "SELECT id, start_date, end_date FROM employment
         WHERE employer_id = $1 AND is_void = FALSE",
    )
    .bind(employer_id.as_str())
    .fetch_all(&mut *tx)
    .await?;

    for (employment_id, start_date, end_date) in employments {
        if overlaps(period, start_date, end_date) {
            sqlx::query(
                "INSERT INTO payroll_run_employment (payroll_run_id, employment_id)
                 VALUES ($1::uuid, $2)",
            )
            .bind(run_id.as_str())
            .bind(&employment_id)
            .execute(&mut *tx)
            .await?;
        }
    }

    write_action_log_entry(
        &mut tx,
        ActionLogEntry {
            employer_id,
            actor: created_by,
            action_type: ActionType::PayrollRunCreated,
            target_type: "payroll_run",
            target_id: run_id.as_str(),
            context: None,
        },
    )
    .await?;

    tx.commit().await?;
    Ok(run_id)
}

/// Validates that `period` is a `PayPeriod` `schedule` itself generates —
/// both boundaries, not just the end date, because a run whose `period_end`
/// is right and whose `period_start` is not would still key correctly in
/// the Ordinary uniqueness index while pricing a span nobody works.
///
/// Decided in Rust against the pure crate's `PaySchedule`, never as SQL
/// date arithmetic, for the same reason `record_compensation_terms` and
/// `record_opening_balance` decide their boundaries there: the schedule's
/// month arithmetic exists once.
fn validate_period_is_one_the_schedule_generates(
    schedule: PaySchedule,
    period: PayPeriod,
) -> Result<(), PayrollAppError> {
    let schedules_period = schedule
        .period_containing(period.end())
        .ok_or(PayrollError::PayScheduleOutsideRepresentableCalendar { date: period.end() })?;
    if schedules_period != period {
        return Err(PayrollAppError::PayPeriodNotGeneratedByThePaySchedule {
            period,
            schedules_period,
        });
    }
    Ok(())
}

/// Whether an Employment spanning `[start_date, end_date]` overlaps
/// `period`. `end_date` of `None` means still employed, so it overlaps
/// everything from `start_date` onward.
fn overlaps(period: PayPeriod, start_date: NaiveDate, end_date: Option<NaiveDate>) -> bool {
    start_date <= period.end() && end_date.is_none_or(|end| end >= period.start())
}

/// Locks a run and verifies it is still in the one lifecycle state where
/// working state may change (§4.7). The lock also prevents a finalizer from
/// moving the same run to `Finalized` between this check and the mutation.
async fn lock_draft_run(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payroll_run_id: &PayrollRunId,
) -> Result<String, PayrollAppError> {
    let run: Option<(String, String)> =
        sqlx::query_as("SELECT kind, status FROM payroll_run WHERE id = $1::uuid FOR UPDATE")
            .bind(payroll_run_id.as_str())
            .fetch_optional(&mut **tx)
            .await?;

    let Some((kind, status)) = run else {
        return Err(PayrollAppError::PayrollRunNotFound(payroll_run_id.clone()));
    };
    if status != "draft" {
        return Err(PayrollAppError::PayrollRunNotDraft(payroll_run_id.clone()));
    }
    Ok(kind)
}

/// Removes `employment_id` from `payroll_run_id`'s working membership.
/// Demands a `reason` that is not blank — checked in Rust before anything
/// is written, though `payroll_run_employment`'s own CHECKs (migrations
/// 0012 and 0023) would refuse an empty or whitespace-only one regardless —
/// and records `actor` and the time
/// as `removed_by`/`removed_at` in the same statement that clears the
/// membership.
///
/// The `removed_at IS NULL` predicate is what makes two concurrent removals
/// resolve to one, exactly as `void_employment`'s `is_void = FALSE`
/// predicate does: a member already removed, or an Employment that was
/// never a member of this run at all, is refused rather than silently
/// overwriting who removed it and why.
pub async fn remove_employment_from_run(
    pool: &PgPool,
    payroll_run_id: &PayrollRunId,
    employment_id: &EmploymentId,
    reason: &str,
    actor: &str,
) -> Result<(), PayrollAppError> {
    // Whitespace, not just `""`: a reason of `" "` is a reason nobody can
    // read six months later, and the ActionLog it lands in cannot be
    // corrected afterwards.
    if reason.trim().is_empty() {
        return Err(PayrollAppError::RemovalReasonCannotBeEmpty);
    }

    let mut tx = pool.begin().await?;
    let kind = lock_draft_run(&mut tx, payroll_run_id).await?;
    if kind != "ordinary" {
        return Err(PayrollAppError::PayrollRunIsNotOrdinary(
            payroll_run_id.clone(),
        ));
    }

    let employer_id: Option<String> = sqlx::query_scalar(
        "UPDATE payroll_run_employment
         SET removed_at = now(), removed_by = $3, removal_reason = $4
         FROM payroll_run
         WHERE payroll_run_employment.payroll_run_id = $1::uuid
           AND payroll_run_employment.employment_id = $2
           AND payroll_run_employment.removed_at IS NULL
           AND payroll_run.id = payroll_run_employment.payroll_run_id
         RETURNING payroll_run.employer_id",
    )
    .bind(payroll_run_id.as_str())
    .bind(employment_id.as_str())
    .bind(actor)
    .bind(reason)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(employer_id) = employer_id else {
        return Err(PayrollAppError::EmploymentNotAnActiveRunMember {
            payroll_run_id: payroll_run_id.clone(),
            employment_id: employment_id.clone(),
        });
    };
    let employer_id = EmployerId::new(employer_id);

    write_action_log_entry(
        &mut tx,
        ActionLogEntry {
            employer_id: &employer_id,
            actor,
            action_type: ActionType::EmploymentRemovedFromRun,
            target_type: "employment",
            target_id: employment_id.as_str(),
            context: Some(serde_json::json!({ "reason": reason })),
        },
    )
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Replaces the classified `Earning` lines held for `(payroll_run_id,
/// employment_id)` with `earnings`, in the order given (§4.5d). An empty
/// `earnings` is a complete statement — no additional Earnings this period —
/// and clears whatever was there, rather than being refused or ignored:
/// Earnings are the Employer's own act of paying, so there is no unasked
/// question here to confirm-none the way `PriorEmployment` and
/// `UnsupportedDeductionStatus` have one.
///
/// A `BasicPay` line is refused. `calculate` derives `BasicPay` itself from
/// the Employment's `CompensationTerms` — it is also the social security
/// base — so a second one supplied here would silently double it.
///
/// An Employment that is not an *active* member of the run is refused too:
/// one that was never proposed, and one that was removed with a reason.
pub async fn set_run_earnings(
    pool: &PgPool,
    payroll_run_id: &PayrollRunId,
    employment_id: &EmploymentId,
    earnings: Vec<Earning>,
) -> Result<(), PayrollAppError> {
    if earnings
        .iter()
        .any(|earning| matches!(earning, Earning::BasicPay(_)))
    {
        return Err(PayrollAppError::BasicPayCannotBeSetAsAnEarning);
    }

    let mut tx = pool.begin().await?;
    lock_draft_run(&mut tx, payroll_run_id).await?;

    // Earning lines are a fact about paying this Employment for this
    // period, so a run that is not paying it has nowhere to put them. The
    // membership foreign key from migration 0017 already refuses an
    // Employment that was never proposed, but it cannot see `removed_at`:
    // without this check, lines could be written against someone the
    // Employer has deliberately, reasonedly taken out of the run, and they
    // would sit there looking like pay that was intended.
    //
    // No row lock is needed here. `remove_employment_from_run` takes the
    // run's own `FOR UPDATE` before it removes anything, and `lock_draft_run`
    // above holds that same lock, so a removal cannot commit between this
    // read and the writes below.
    let is_active_member: Option<bool> = sqlx::query_scalar(
        "SELECT TRUE FROM payroll_run_employment
         WHERE payroll_run_id = $1::uuid AND employment_id = $2 AND removed_at IS NULL",
    )
    .bind(payroll_run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    if is_active_member.is_none() {
        return Err(PayrollAppError::EmploymentNotAnActiveRunMember {
            payroll_run_id: payroll_run_id.clone(),
            employment_id: employment_id.clone(),
        });
    }

    // Replace, not merge: the whole point of §4.5d is that this call states
    // the complete list, so a prior call's leftover lines must not survive
    // alongside a shorter new list.
    sqlx::query(
        "DELETE FROM payroll_run_earning WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(payroll_run_id.as_str())
    .bind(employment_id.as_str())
    .execute(&mut *tx)
    .await?;

    for (index, earning) in earnings.iter().enumerate() {
        let line = i16::try_from(index)
            .expect("a payroll run holds far fewer than i16::MAX earning lines");
        let earning_json = serde_json::to_value(earning).expect("Earning always serializes");
        sqlx::query(
            "INSERT INTO payroll_run_earning (payroll_run_id, employment_id, line, earning_json)
             VALUES ($1::uuid, $2, $3, $4)",
        )
        .bind(payroll_run_id.as_str())
        .bind(employment_id.as_str())
        .bind(line)
        .bind(earning_json)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn period() -> PayPeriod {
        PayPeriod::new(date(2026, 1, 26), date(2026, 2, 25)).unwrap()
    }

    fn schedule() -> PaySchedule {
        PaySchedule::new(payroll::PeriodEndDay::Day(
            payroll::DayOfMonth::new(25).unwrap(),
        ))
    }

    #[test]
    fn the_schedules_own_period_is_accepted() {
        assert_eq!(
            validate_period_is_one_the_schedule_generates(schedule(), period()),
            Ok(())
        );
    }

    #[test]
    fn a_calendar_month_is_refused_by_a_twenty_sixth_to_twenty_fifth_schedule() {
        let calendar_february = PayPeriod::new(date(2026, 2, 1), date(2026, 2, 28)).unwrap();

        assert_eq!(
            validate_period_is_one_the_schedule_generates(schedule(), calendar_february),
            Err(PayrollAppError::PayPeriodNotGeneratedByThePaySchedule {
                period: calendar_february,
                schedules_period: PayPeriod::new(date(2026, 2, 26), date(2026, 3, 25)).unwrap(),
            })
        );
    }

    #[test]
    fn a_period_with_the_right_end_and_a_wrong_start_is_refused() {
        let wrong_start = PayPeriod::new(date(2026, 2, 1), date(2026, 2, 25)).unwrap();

        assert_eq!(
            validate_period_is_one_the_schedule_generates(schedule(), wrong_start),
            Err(PayrollAppError::PayPeriodNotGeneratedByThePaySchedule {
                period: wrong_start,
                schedules_period: period(),
            })
        );
    }

    #[test]
    fn a_continuing_employment_overlaps() {
        assert!(overlaps(period(), date(2025, 1, 1), None));
    }

    #[test]
    fn an_employment_wholly_before_the_period_does_not_overlap() {
        assert!(!overlaps(
            period(),
            date(2024, 1, 1),
            Some(date(2026, 1, 25))
        ));
    }

    #[test]
    fn an_employment_wholly_after_the_period_does_not_overlap() {
        assert!(!overlaps(period(), date(2026, 2, 26), None));
    }

    #[test]
    fn an_employment_starting_on_the_periods_last_day_overlaps() {
        assert!(overlaps(period(), date(2026, 2, 25), None));
    }

    #[test]
    fn an_employment_ending_on_the_periods_first_day_overlaps() {
        assert!(overlaps(
            period(),
            date(2025, 1, 1),
            Some(date(2026, 1, 26))
        ));
    }
}
