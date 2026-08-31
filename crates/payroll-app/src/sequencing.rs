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

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;
use payroll::{EmployerId, EmploymentId, PayPeriod, PaySchedule, TaxYear};

use crate::error::PayrollAppError;
use crate::payroll_run::EmploymentSpan;

/// Refuses unless `period`'s immediately preceding `PayPeriod` — in the same
/// TaxYear — is resolved (§7.1) for every one of `members`. This is
/// §5.3 step 3's Ordinary column; a Correction run checks its own period
/// alone and never walks back (§7.4), so it does not call this.
///
/// No predecessor to check is not a refusal, and is not distinguished from
/// one another: `period` may be the TaxYear's own first period, or the
/// schedule's `preceding_period` may find nothing at the very edge of the
/// representable calendar. Either way there is nothing before `period` this
/// check is responsible for.
///
/// `tax_year` is `period`'s own, passed in rather than re-derived, because
/// the caller has already resolved the run's rules from it — one TaxYear
/// governs the whole finalization, and two derivations of it could only
/// ever disagree by accident.
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
    tax_year: TaxYear,
    members: &[EmploymentSpan],
) -> Result<(), PayrollAppError> {
    let Some(preceding) = schedule.preceding_period(period) else {
        return Ok(());
    };

    if TaxYear::for_period_end(preceding.end()) != tax_year {
        // The walk stops at the TaxYear boundary (§7.2): `period` is that
        // TaxYear's own first period under this schedule, so nothing before
        // it is this check's concern.
        return Ok(());
    }

    let member_ids: Vec<String> = members.iter().map(|member| member.id.clone()).collect();
    let records =
        RecordsForThePrecedingPeriod::read(tx, employer_id, tax_year, preceding.end(), &member_ids)
            .await?;

    for member in members {
        if !records.resolve(member, preceding) {
            return Err(PayrollAppError::PrecedingPeriodUnresolved {
                employment_id: EmploymentId::new(member.id.clone()),
                period: preceding,
            });
        }
    }
    Ok(())
}

/// Every record §7.1 branches 2-4 ask about, for the whole membership at
/// once: four statements, whatever the member count, rather than up to four
/// per member inside the finalization transaction.
///
/// Reading all four up front — instead of stopping at the first branch that
/// answers for a given member — costs nothing a run of any size notices, and
/// keeps the branch order a property of [`Self::resolve`] alone, where §7.1
/// states it.
struct RecordsForThePrecedingPeriod {
    /// Branch 2: `(E, Y)`'s `SaltCoverageStart`, for the members that have
    /// an `OpeningBalance` for this TaxYear at all.
    salt_coverage_start: HashMap<String, NaiveDate>,
    /// Branch 3: the members with a live `FinalizedPayroll` for the period.
    paid: HashSet<String>,
    /// Branch 4, first half: the members removed with a mandatory reason
    /// from the finalized Ordinary run for the period.
    removed_with_a_reason: HashSet<String>,
    /// Branch 4, second half: the members Salt holds at least one
    /// `FinalizedPayroll` for, every one of which carries a `Reversal`.
    every_payroll_reversed: HashSet<String>,
}

impl RecordsForThePrecedingPeriod {
    async fn read(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        employer_id: &EmployerId,
        tax_year: TaxYear,
        period_end: NaiveDate,
        member_ids: &[String],
    ) -> Result<Self, PayrollAppError> {
        let salt_coverage_start: Vec<(String, NaiveDate)> = sqlx::query_as(
            "SELECT employment_id, first_salt_period_end FROM opening_balance
             WHERE employment_id = ANY($1) AND tax_year = $2",
        )
        .bind(member_ids)
        .bind(tax_year.starting_year())
        .fetch_all(&mut **tx)
        .await?;

        let paid: Vec<String> = sqlx::query_scalar(
            "SELECT employment_id FROM live_finalized_payroll
             WHERE employment_id = ANY($1) AND period_end = $2",
        )
        .bind(member_ids)
        .bind(period_end)
        .fetch_all(&mut **tx)
        .await?;

        // A removal from a run that has not itself finalized says nothing
        // yet — the run could still be recalculated with the member added
        // back, or never finalized at all — so `status = 'finalized'` is
        // part of the question, not an incidental filter.
        let removed_with_a_reason: Vec<String> = sqlx::query_scalar(
            "SELECT payroll_run_employment.employment_id
             FROM payroll_run_employment
             JOIN payroll_run ON payroll_run.id = payroll_run_employment.payroll_run_id
             WHERE payroll_run_employment.employment_id = ANY($1)
               AND payroll_run.employer_id = $2
               AND payroll_run.period_end = $3
               AND payroll_run.kind = 'ordinary'
               AND payroll_run.status = 'finalized'
               AND payroll_run_employment.removed_at IS NOT NULL",
        )
        .bind(member_ids)
        .bind(employer_id.as_str())
        .bind(period_end)
        .fetch_all(&mut **tx)
        .await?;

        // §7.1 branch 4 says *every* `FinalizedPayroll` for the period has
        // been reversed, so the `Reversal` rows are what this asks for. The
        // absence of a `live_finalized_payroll` row would be a cheaper
        // stand-in and is one today, but it is an absence — and an absence
        // never resolves a period (§7.1). `reversal` holds at most one row
        // per `FinalizedPayroll` (migration 0011), so the join cannot fan
        // out and the count is exactly "payrolls still unreversed".
        let every_payroll_reversed: Vec<String> = sqlx::query_scalar(
            "SELECT finalized_payroll.employment_id
             FROM finalized_payroll
             LEFT JOIN reversal ON reversal.finalized_payroll_id = finalized_payroll.id
             WHERE finalized_payroll.employment_id = ANY($1)
               AND finalized_payroll.period_end = $2
             GROUP BY finalized_payroll.employment_id
             HAVING count(*) FILTER (WHERE reversal.id IS NULL) = 0",
        )
        .bind(member_ids)
        .bind(period_end)
        .fetch_all(&mut **tx)
        .await?;

        Ok(Self {
            salt_coverage_start: salt_coverage_start.into_iter().collect(),
            paid: paid.into_iter().collect(),
            removed_with_a_reason: removed_with_a_reason.into_iter().collect(),
            every_payroll_reversed: every_payroll_reversed.into_iter().collect(),
        })
    }

    /// Whether `preceding` is resolved for `member` — §7.1's four branches,
    /// asked in the order the section states them. The branches are not
    /// mutually exclusive; whichever answers first is why this stops
    /// looking, not a claim that the others do not also hold.
    fn resolve(&self, member: &EmploymentSpan, preceding: PayPeriod) -> bool {
        // Branch 1: outside the Employment. The exact predicate
        // `create_ordinary_payroll_run` uses to decide membership itself.
        if !member.overlaps(preceding) {
            return true;
        }

        // Branch 2: before Salt. An `OpeningBalance` for (E, Y) puts
        // `preceding` inside the figures it carries, once its boundary
        // falls strictly after `preceding`'s own end — a boundary *on* that
        // end makes `preceding` Salt's own first period, which branch 3 or
        // branch 4 answers for, not this one.
        if self
            .salt_coverage_start
            .get(&member.id)
            .is_some_and(|coverage_start| preceding.end() < *coverage_start)
        {
            return true;
        }

        // Branch 3: paid.
        if self.paid.contains(&member.id) {
            return true;
        }

        // Branch 4: explicitly not paid — a reasoned removal from the
        // finalized Ordinary run, or every `FinalizedPayroll` for the period
        // reversed with none live. A bare reversal resolves its period; it
        // is not a gap (§7.1).
        self.removed_with_a_reason.contains(&member.id)
            || (self.every_payroll_reversed.contains(&member.id) && !self.paid.contains(&member.id))
    }
}
