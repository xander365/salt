//! §7's ordering and sequencing rule: whether a `PayPeriod` is *resolved*
//! for an Employment, and the one-period walk-back an Ordinary run's
//! finalization performs before it rebuilds anything (§5.3 step 3).
//!
//! Payroll order is `PayPeriod.end()`, never a finalization timestamp — a
//! late-finalized March is still March, so every query below reads
//! `period_end`, not `finalized_at`.
//!
//! ### The four branches (§7.1)
//!
//! For Employment E and PayPeriod P in TaxYear Y, P is resolved for E when
//! at least one holds:
//!
//! 1. **Outside the Employment** — P does not overlap E's employment span.
//!    Nothing was owed.
//! 2. **Before Salt** — an `OpeningBalance` exists for (E, Y) and P's end is
//!    before its `SaltCoverageStart`. P's figures are inside that balance.
//! 3. **Paid** — a live `FinalizedPayroll` exists for (E, P.end).
//! 4. **Explicitly not paid** — either E was removed with a mandatory
//!    reason from the finalized Ordinary run for P, or every
//!    `FinalizedPayroll` Salt holds for (E, P.end) has been reversed and
//!    none is live.
//!
//! Nothing else resolves a period. Absence of a record never resolves one —
//! there is deliberately no `NoPayrollRecord` concept anywhere in this
//! crate: a `Reversal` and a reasoned removal already record "this period
//! was explicitly not paid", and a third way to say the same thing is how
//! all three would drift apart.
//!
//! ### One period back is enough
//!
//! Resolution is inductive. Branches 1 and 2 each cover every earlier
//! period by construction — an Employment's `start_date` does not move
//! backwards, and every period before a frozen `SaltCoverageStart` is
//! inside the same balance. Branches 3 and 4 mean that period was itself
//! finalized, and its own finalization ran this very check against *its*
//! predecessor. So checking one period back proves every earlier one in the
//! TaxYear already holds, and nothing can un-resolve a period afterwards: a
//! reversal still resolves it (branch 4), a removal is immutable, and an
//! `OpeningBalance` boundary froze at the Employment's first finalization
//! in that TaxYear (§4.5, ADR-0013).
//!
//! The walk stops at the TaxYear's own first period: PAYE resets there
//! (ADR-0001), so a TaxYear Salt never touched cannot block the one it
//! does.

use chrono::NaiveDate;
use payroll::{EmployerId, EmploymentId, PayPeriod, PaySchedule, TaxYear};

use crate::error::PayrollAppError;
use crate::payroll_run::overlaps;

/// Refuses unless `period`'s immediately preceding `PayPeriod` — in the same
/// TaxYear — is resolved (§7.1) for every one of `member_ids`. This is
/// §5.3 step 3's Ordinary column; a Correction run checks its own period
/// alone and never walks back (§7.4), so it does not call this.
///
/// No predecessor to check is not a refusal, and is not distinguished from
/// one another: `period` may be the TaxYear's own first period, or the
/// schedule's `preceding_period` may find nothing at the very edge of the
/// representable calendar. Either way there is nothing before `period` this
/// check is responsible for.
///
/// Takes no lock of its own. The `FOR SHARE` `finalize_payroll_run` has
/// already taken on every member's `employment` row before calling this is
/// what a concurrent `OpeningBalance` or `PriorEmployment` write conflicts
/// with (§5.4); the preceding period's own `FinalizedPayroll`, once
/// written, never changes underneath a read of it (REVOKE UPDATE, DELETE,
/// migration 0015), and a still-open predecessor run cannot be finalized
/// concurrently with this one without first acquiring the very same
/// `payroll_run` lock this function's caller already holds on the *current*
/// run — the predecessor's own lock is its own finalization's problem, not
/// this read's.
pub(crate) async fn verify_the_preceding_period_is_resolved_for_every_member(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    employer_id: &EmployerId,
    schedule: PaySchedule,
    period: PayPeriod,
    member_ids: &[String],
) -> Result<(), PayrollAppError> {
    let Some(preceding) = schedule.preceding_period(period) else {
        return Ok(());
    };

    let tax_year = TaxYear::for_period_end(period.end());
    if TaxYear::for_period_end(preceding.end()) != tax_year {
        // The walk stops at the TaxYear boundary (§7.2): `period` is that
        // TaxYear's own first period under this schedule, so nothing before
        // it is this check's concern.
        return Ok(());
    }

    for member_id in member_ids {
        let employment_id = EmploymentId::new(member_id.clone());
        if !preceding_period_is_resolved(tx, &employment_id, employer_id, tax_year, preceding)
            .await?
        {
            return Err(PayrollAppError::PrecedingPeriodUnresolved {
                employment_id,
                period: preceding,
            });
        }
    }
    Ok(())
}

/// Whether `preceding` is resolved for `employment_id` — §7.1's four
/// branches, checked in the order the section states them. The branches are
/// not mutually exclusive; whichever is found first is why this stops
/// looking, not a claim that the others do not also hold.
async fn preceding_period_is_resolved(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    employment_id: &EmploymentId,
    employer_id: &EmployerId,
    tax_year: TaxYear,
    preceding: PayPeriod,
) -> Result<bool, PayrollAppError> {
    // Branch 1: outside the Employment. The exact predicate
    // `create_ordinary_payroll_run` uses to decide membership itself.
    let (start_date, end_date): (NaiveDate, Option<NaiveDate>) =
        sqlx::query_as("SELECT start_date, end_date FROM employment WHERE id = $1")
            .bind(employment_id.as_str())
            .fetch_one(&mut **tx)
            .await?;
    if !overlaps(preceding, start_date, end_date) {
        return Ok(true);
    }

    // Branch 2: before Salt. An `OpeningBalance` for (E, Y) puts `preceding`
    // inside the figures it carries, once its boundary is at or after
    // `preceding`'s own end.
    let salt_coverage_start: Option<NaiveDate> = sqlx::query_scalar(
        "SELECT first_salt_period_end FROM opening_balance
         WHERE employment_id = $1 AND tax_year = $2",
    )
    .bind(employment_id.as_str())
    .bind(tax_year.starting_year())
    .fetch_optional(&mut **tx)
    .await?;
    if salt_coverage_start.is_some_and(|coverage_start| preceding.end() < coverage_start) {
        return Ok(true);
    }

    // Branch 3: paid. A live `FinalizedPayroll` for (E, preceding.end()).
    let live: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM live_finalized_payroll
            WHERE employment_id = $1 AND period_end = $2
         )",
    )
    .bind(employment_id.as_str())
    .bind(preceding.end())
    .fetch_one(&mut **tx)
    .await?;
    if live {
        return Ok(true);
    }

    // Branch 4, second half: every `FinalizedPayroll` Salt holds for
    // (E, preceding.end()) has been reversed. `live` is already known false
    // above, so any row found here is necessarily reversed — a bare
    // reversal resolves its period; it is not a gap (§7.1).
    let ever_finalized: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM finalized_payroll
            WHERE employment_id = $1 AND period_end = $2
         )",
    )
    .bind(employment_id.as_str())
    .bind(preceding.end())
    .fetch_one(&mut **tx)
    .await?;
    if ever_finalized {
        return Ok(true);
    }

    // Branch 4, first half: E was removed with a mandatory reason from the
    // finalized Ordinary run for `preceding`. A removal from a run that has
    // not itself finalized says nothing yet — the run could still be
    // recalculated with the member added back, or never finalized at all —
    // so `payroll_run.status = 'finalized'` is part of the question, not an
    // incidental filter.
    let removed_with_reason: bool = sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM payroll_run_employment
            JOIN payroll_run ON payroll_run.id = payroll_run_employment.payroll_run_id
            WHERE payroll_run_employment.employment_id = $1
              AND payroll_run.employer_id = $2
              AND payroll_run.period_end = $3
              AND payroll_run.kind = 'ordinary'
              AND payroll_run.status = 'finalized'
              AND payroll_run_employment.removed_at IS NOT NULL
         )",
    )
    .bind(employment_id.as_str())
    .bind(employer_id.as_str())
    .bind(preceding.end())
    .fetch_one(&mut **tx)
    .await?;

    Ok(removed_with_reason)
}
