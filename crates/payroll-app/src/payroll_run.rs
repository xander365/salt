//! `CreateOrdinaryPayrollRun`, `RemoveEmploymentFromRun` and
//! `SetRunEarnings` — the Ordinary half of §4.6-§4.8 and §4.5d, §12.
//! `CreateCorrectionRun`, calculation and finalization are separate, later
//! use cases: Ordinary and Correction membership are opposites (§4.8,
//! ADR-0015), so a single entry point taking a `kind` would branch on its
//! first line and share nothing after it.

use chrono::NaiveDate;
use payroll::{Earning, EmployerId, EmploymentId, PayPeriod};
use sqlx::PgPool;

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
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

    let id: Option<String> = sqlx::query_scalar(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status, created_by)
         SELECT $1, $2, $3, $4, 'ordinary', 'draft', $5
         WHERE EXISTS (SELECT 1 FROM employer WHERE id = $1)
         RETURNING id::text",
    )
    .bind(employer_id.as_str())
    .bind(period.start())
    .bind(period.end())
    .bind(pay_date)
    .bind(created_by)
    .fetch_optional(&mut *tx)
    .await?;
    let run_id = PayrollRunId::new(
        id.ok_or_else(|| PayrollAppError::EmployerNotFound(employer_id.clone()))?,
    );

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

/// Whether an Employment spanning `[start_date, end_date]` overlaps
/// `period`. `end_date` of `None` means still employed, so it overlaps
/// everything from `start_date` onward.
fn overlaps(period: PayPeriod, start_date: NaiveDate, end_date: Option<NaiveDate>) -> bool {
    start_date <= period.end() && end_date.is_none_or(|end| end >= period.start())
}

/// Removes `employment_id` from `payroll_run_id`'s working membership.
/// Demands a non-empty `reason` — checked in Rust before anything is
/// written, though `payroll_run_employment`'s own CHECK (migration 0012)
/// would refuse a blank one regardless — and records `actor` and the time
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
    if reason.is_empty() {
        return Err(PayrollAppError::RemovalReasonCannotBeEmpty);
    }

    let mut tx = pool.begin().await?;

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
