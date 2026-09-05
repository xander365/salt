//! `CreateOrdinaryPayrollRun`, `RemoveEmploymentFromRun` and
//! `SetRunEarnings` — the Ordinary half of §4.6-§4.8 and §4.5d, §12.
//! `CreateCorrectionRun`, calculation and finalization are separate, later
//! use cases: Ordinary and Correction membership are opposites (§4.8,
//! ADR-0015), so a single entry point taking a `kind` would branch on its
//! first line and share nothing after it.

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::database::SaltDatabase;
use crate::employer::{generates_the_period_end, pay_schedule_from_columns};
use crate::employment::get_employment_snapshot_on;
use crate::error::PayrollAppError;
use crate::finalize::FinalizedPayrollId;
use crate::freeze::finalized_period_ends_in;
use crate::ids::app_id;
use crate::prior_employment::get_prior_employment_on;
use crate::unsupported_deduction_status::get_unsupported_deduction_status_on;
use chrono::NaiveDate;
use payroll::{
    Deduction, Earning, EmployerId, EmploymentId, Money, PayPeriod, PaySchedule,
    PayrollCalculation, PayrollError, PriorEmployment, PriorEmploymentFigures, TaxYear,
    UnsupportedDeductionKinds, UnsupportedDeductionStatus,
};

app_id! {
    /// `payroll-app`'s own id (§4.1): a native UUID, unlike the pure crate's
    /// opaque `TEXT`-backed ids. Minted only by `create_ordinary_payroll_run`,
    /// where the row itself is inserted.
    PayrollRunId
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
///
/// The run's TaxYear must also still be one the current `PaySchedule`
/// describes: every period already finalized in it has to be a period end
/// that schedule generates (§4.2, ADR-0005). `change_pay_schedule` refuses a
/// mid-year change, but it can only check the TaxYear the caller *claims* to
/// be in; this check needs no such claim, so a schedule moved under a
/// part-finalized TaxYear is caught here even if the change itself slipped
/// through. It is what actually holds "a TaxYear never contains other than
/// twelve periods": no further period of that year can be run.
pub async fn create_ordinary_payroll_run(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    period: PayPeriod,
    pay_date: NaiveDate,
    created_by: &str,
) -> Result<PayrollRunId, PayrollAppError> {
    let mut tx = db.pool().begin().await?;

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

    let schedule = pay_schedule_from_columns(&kind, value);
    validate_period_is_one_the_schedule_generates(schedule, period)?;
    validate_the_schedule_still_describes_the_tax_year(
        &mut tx,
        employer_id,
        schedule,
        TaxYear::for_period_end(period.end()),
    )
    .await?;

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

/// Creates a Draft Correction `PayrollRun` for `employer_id` and `period`,
/// with its own `pay_date` and a mandatory, non-empty `correction_reason`
/// (§4.6, §4.8, ADR-0015). Proposes nobody: unlike an Ordinary run, nothing
/// is auto-included — `add_employment_to_correction_run` is the only way a
/// member joins, and it accepts exactly one.
///
/// `correction_reason` is checked here, before anything is written, though
/// `payroll_run`'s own CHECK (migration 0007) would refuse an empty one
/// regardless — the same belt-and-braces `remove_employment_from_run` and
/// `reverse_finalized_payroll` already apply to their own mandatory reasons.
///
/// Unlike [`create_ordinary_payroll_run`], `period` is not checked against
/// the Employer's `PaySchedule`: a Correction run corrects a period that was
/// already run once, under whatever schedule was in force then, and nothing
/// downstream — no walk-back (§7.4), no uniqueness index keyed on it — needs
/// it to be one the *current* schedule still generates.
pub async fn create_correction_run(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    period: PayPeriod,
    pay_date: NaiveDate,
    correction_reason: &str,
    created_by: &str,
) -> Result<PayrollRunId, PayrollAppError> {
    if correction_reason.trim().is_empty() {
        return Err(PayrollAppError::CorrectionReasonCannotBeEmpty);
    }

    let mut tx = db.pool().begin().await?;

    let employer_exists: Option<bool> =
        sqlx::query_scalar("SELECT TRUE FROM employer WHERE id = $1")
            .bind(employer_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
    if employer_exists.is_none() {
        return Err(PayrollAppError::EmployerNotFound(employer_id.clone()));
    }

    let id: String = sqlx::query_scalar(
        "INSERT INTO payroll_run
            (employer_id, period_start, period_end, pay_date, kind, status,
             correction_reason, created_by)
         VALUES ($1, $2, $3, $4, 'correction', 'draft', $5, $6)
         RETURNING id::text",
    )
    .bind(employer_id.as_str())
    .bind(period.start())
    .bind(period.end())
    .bind(pay_date)
    .bind(correction_reason)
    .bind(created_by)
    .fetch_one(&mut *tx)
    .await?;
    let run_id = PayrollRunId::new(id);

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

/// Refuses a run in a TaxYear whose already finalized periods `schedule` no
/// longer generates — proof the Employer's `PaySchedule` moved inside a
/// TaxYear that had already finalized payroll. Only the finalized periods
/// count, for the same reason the freeze itself counts them: they are the
/// ones a later period's cumulative PAYE is built on (ADR-0001), and they
/// cannot be re-cut.
async fn validate_the_schedule_still_describes_the_tax_year(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    employer_id: &EmployerId,
    schedule: PaySchedule,
    tax_year: TaxYear,
) -> Result<(), PayrollAppError> {
    for finalized_period_end in finalized_period_ends_in(tx, employer_id, tax_year).await? {
        if !generates_the_period_end(schedule, finalized_period_end) {
            return Err(PayrollAppError::PayScheduleMovedWithinTaxYear {
                employer_id: employer_id.clone(),
                tax_year,
                finalized_period_end,
            });
        }
    }
    Ok(())
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

/// One member's Employment span, read once under the `FOR SHARE` lock
/// `finalize_payroll_run` takes over every member's `employment` row (§5.4)
/// and passed on from there, so no later step re-reads a row it is already
/// holding.
///
/// [`crate::sequencing`] decides §7.1 branch 1 — "P does not overlap the
/// Employment" — from this, with the same [`overlaps`] predicate membership
/// itself is decided by.
#[derive(sqlx::FromRow)]
pub(crate) struct EmploymentSpan {
    pub(crate) id: String,
    start_date: NaiveDate,
    end_date: Option<NaiveDate>,
}

impl EmploymentSpan {
    /// Whether this Employment overlaps `period` — see [`overlaps`].
    pub(crate) fn overlaps(&self, period: PayPeriod) -> bool {
        overlaps(period, self.start_date, self.end_date)
    }
}

/// Takes the run's `FOR UPDATE` lock and reads back the columns every
/// lifecycle decision is made on. A run that does not exist is refused here,
/// so no caller has to spell that out; **which** states the caller accepts is
/// the caller's own question, and it answers it from the returned `status`.
///
/// The lock is what serialises everything that touches one run — two
/// removals, a removal against a recalculation, a recalculation against a
/// finalizer — so it is taken before any state is judged, never after.
pub(crate) async fn lock_run(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payroll_run_id: &PayrollRunId,
) -> Result<LockedRun, PayrollAppError> {
    let run: Option<(String, String, NaiveDate, NaiveDate, String, Option<String>)> =
        sqlx::query_as(
            "SELECT kind, status, period_start, period_end, employer_id, correction_reason
         FROM payroll_run WHERE id = $1::uuid FOR UPDATE",
        )
        .bind(payroll_run_id.as_str())
        .fetch_optional(&mut **tx)
        .await?;

    let Some((kind, status, period_start, period_end, employer_id, correction_reason)) = run else {
        return Err(PayrollAppError::PayrollRunNotFound(payroll_run_id.clone()));
    };
    Ok(LockedRun {
        kind: RunKind::from_column(&kind),
        status: RunStatus::from_column(&status),
        period: PayPeriod::new(period_start, period_end)
            .expect("payroll_run CHECK: period_end is never before period_start"),
        employer_id: EmployerId::new(employer_id),
        correction_reason,
    })
}

/// One `payroll_run` row, read under its own `FOR UPDATE` lock.
pub(crate) struct LockedRun {
    pub kind: RunKind,
    pub status: RunStatus,
    pub period: PayPeriod,
    pub employer_id: EmployerId,
    /// `payroll_run.correction_reason` — always `Some` for a Correction run
    /// and always `None` for an Ordinary one (§4.6's CHECK). Read here so
    /// `add_employment_to_correction_run` can carry it into its own
    /// `ActionLog` entry without a second read of a row this lock already
    /// holds.
    pub correction_reason: Option<String>,
}

/// The two kinds of run §4.6 names, as a type rather than the `TEXT` the
/// column holds — every lifecycle decision made on a bare string is one
/// typo away from silently taking the wrong branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunKind {
    Ordinary,
    Correction,
}

impl RunKind {
    /// Panics rather than returning a `Result`, exactly as
    /// [`crate::employer::pay_schedule_from_columns`] does: `payroll_run`'s
    /// own CHECK admits these two values only, so a third would mean the
    /// schema no longer matches this code, not a fact about the run.
    fn from_column(kind: &str) -> Self {
        match kind {
            "ordinary" => Self::Ordinary,
            "correction" => Self::Correction,
            other => {
                panic!("payroll_run CHECK: kind is 'ordinary' or 'correction', found {other:?}")
            }
        }
    }
}

/// The three states §4.7 names. `Draft → Calculated → Finalized`; there is
/// no `Reviewed` (ADR-0010). Public — issue #53's read models hand it
/// straight to `salt-server`, which turns it into its own wire string rather
/// than this crate doing so on their behalf (the same split
/// `MembershipStatus` already draws).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Draft,
    Calculated,
    Finalized,
}

impl RunStatus {
    /// Panics for the same reason [`RunKind::from_column`] does.
    fn from_column(status: &str) -> Self {
        match status {
            "draft" => Self::Draft,
            "calculated" => Self::Calculated,
            "finalized" => Self::Finalized,
            other => panic!(
                "payroll_run CHECK: status is 'draft', 'calculated' or 'finalized', found {other:?}"
            ),
        }
    }
}

/// The Employments still in `payroll_run_id`'s working membership, in a
/// stable order. Read once per use case, by every caller that walks a run's
/// members — recalculation and finalization must see exactly the same set,
/// so they read it through the same query rather than two copies of it that
/// can drift.
pub(crate) async fn active_member_ids(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payroll_run_id: &PayrollRunId,
) -> Result<Vec<String>, PayrollAppError> {
    let member_ids: Vec<String> = sqlx::query_scalar(
        "SELECT employment_id FROM payroll_run_employment
         WHERE payroll_run_id = $1::uuid AND removed_at IS NULL
         ORDER BY employment_id",
    )
    .bind(payroll_run_id.as_str())
    .fetch_all(&mut **tx)
    .await?;
    Ok(member_ids)
}

/// Every `FinalizedPayroll` an already-finalized run produced, paired with
/// the Employment it belongs to and ordered by that Employment's id (issue
/// #50, §0.28). An Ordinary run finalizes each active member into its own
/// separate row, so the answer to "which finalized payroll does this run
/// already have" is a list, not one id; a Correction run holds at most one
/// member (§4.8) and so yields at most one pair.
///
/// Reads the immutable history rows belonging to this run rather than the
/// current liveness index. Reversing one may remove its liveness row and a
/// later Correction may install a different live row for the same Employment
/// and period, but neither operation changes which rows the already-finalized
/// run produced.
pub(crate) async fn finalized_payrolls_for_run(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payroll_run_id: &PayrollRunId,
    period_end: NaiveDate,
) -> Result<Vec<(EmploymentId, FinalizedPayrollId)>, PayrollAppError> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT employment_id, id::text FROM finalized_payroll
         WHERE payroll_run_id = $1::uuid AND period_end = $2
         ORDER BY employment_id",
    )
    .bind(payroll_run_id.as_str())
    .bind(period_end)
    .fetch_all(&mut **tx)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(employment_id, finalized_payroll_id)| {
            (
                EmploymentId::new(employment_id),
                FinalizedPayrollId::new(finalized_payroll_id),
            )
        })
        .collect())
}

/// Locks a run, verifies working state may still change, and puts the run
/// back into `Draft` so the edit about to happen is reflected in its status.
///
/// **`Calculated` is editable, and editing reopens the run.** §4.7's arrow
/// runs `Draft → Calculated → Finalized`, but the state it defines is a
/// property of the members — "every member has a current, successful
/// working calculation" — not a gate the Employer passed through. Seeing the
/// figures is exactly when a wrong Earning or a member who should not be
/// paid becomes visible, so refusing the edit would leave an Employer who
/// spotted a mistake with finalizing it or nothing. The moment a line
/// changes, the stored calculations are no longer current, which is the
/// definition of `Draft`; setting the status back here is what keeps the
/// column honest rather than a claim about calculations that have since
/// gone stale.
///
/// `Finalized` is the one refusal left, and it is absolute: history has
/// been written and working state can no longer change.
///
/// The lock is taken before the status is judged, so a finalizer cannot
/// move the same run to `Finalized` between this check and the mutation.
pub(crate) async fn lock_and_reopen_run(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payroll_run_id: &PayrollRunId,
) -> Result<LockedRun, PayrollAppError> {
    let run = lock_run(tx, payroll_run_id).await?;
    if run.status == RunStatus::Finalized {
        let finalized_payrolls =
            finalized_payrolls_for_run(tx, payroll_run_id, run.period.end()).await?;
        return Err(PayrollAppError::PayrollRunAlreadyFinalized {
            payroll_run_id: payroll_run_id.clone(),
            finalized_payrolls,
        });
    }

    sqlx::query(
        "UPDATE payroll_run SET status = 'draft' WHERE id = $1::uuid AND status <> 'draft'",
    )
    .bind(payroll_run_id.as_str())
    .execute(&mut **tx)
    .await?;

    Ok(run)
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
///
/// The removed member's `WorkingPayrollCalculation`, if it had one, goes
/// with it — a member the Employer has taken out of the run has no current
/// calculation, and the next recalculation can make the run `Calculated`
/// again from the members that remain (§4.7, §4.9).
pub async fn remove_employment_from_run(
    db: &SaltDatabase,
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

    let mut tx = db.pool().begin().await?;
    let run = lock_and_reopen_run(&mut tx, payroll_run_id).await?;
    if run.kind != RunKind::Ordinary {
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

    // A removed member has no calculation, so it must not leave one behind.
    // `lock_and_reopen_run` has already put the run back to `Draft`, but the
    // very next recalculation can make it `Calculated` again from the
    // remaining members alone -- and a `Calculated` run holding a
    // `WorkingPayrollCalculation` for someone the Employer deliberately,
    // reasonedly took out of it is a row that looks current and is not
    // (§4.7, §4.9). `calculate_payroll_run` clears a refused member's row
    // for the same reason; this is the other way a member stops having a
    // current calculation.
    sqlx::query(
        "DELETE FROM working_payroll_calculation
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(payroll_run_id.as_str())
    .bind(employment_id.as_str())
    .execute(&mut *tx)
    .await?;

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
    db: &SaltDatabase,
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

    let mut tx = db.pool().begin().await?;
    lock_and_reopen_run(&mut tx, payroll_run_id).await?;

    // Earning lines are a fact about paying this Employment for this
    // period, so a run that is not paying it has nowhere to put them. The
    // membership foreign key from migration 0017 already refuses an
    // Employment that was never proposed, but it cannot see `removed_at`:
    // without this check, lines could be written against someone the
    // Employer has deliberately, reasonedly taken out of the run, and they
    // would sit there looking like pay that was intended.
    //
    // No row lock is needed here. `remove_employment_from_run` takes the
    // run's own `FOR UPDATE` before it removes anything, and
    // `lock_and_reopen_run` above holds that same lock, so a removal cannot
    // commit between this read and the writes below.
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

/// Wraps a caller-supplied id as a [`PayrollRunId`], refusing a string that
/// is not even a well-formed UUID as [`PayrollAppError::PayrollRunNotFound`]
/// — the same "unknown and cross-Employer are indistinguishable" reasoning
/// ADR-0017 already applies, extended to "not a UUID at all": the `id`
/// column's `::uuid` cast would otherwise fail as a database error, turning
/// a client's malformed path segment into a 500 rather than the 404 it
/// deserves.
fn parse_payroll_run_id(payroll_run_id: &str) -> Result<PayrollRunId, PayrollAppError> {
    if uuid::Uuid::parse_str(payroll_run_id).is_err() {
        return Err(PayrollAppError::PayrollRunNotFound(PayrollRunId::new(
            payroll_run_id,
        )));
    }
    Ok(PayrollRunId::new(payroll_run_id))
}

/// Confirms `payroll_run_id` belongs to `employer_id`, refusing exactly like
/// a missing run when it does not (ADR-0017), and hands back the wrapped
/// [`PayrollRunId`]. Mirrors [`crate::verify_employment_belongs_to_employer`],
/// for the same reason: [`set_run_earnings`] takes no `EmployerId` of its
/// own, so a handler that only holds one from `AuthorizedEmployerContext`
/// needs this check first (issue #53).
///
/// Takes the id as `&str`, not `&PayrollRunId`: like
/// [`crate::load_session`]'s token, this is the one boundary where a
/// caller's own id — read off an HTTP path, never minted here — becomes the
/// wrapped type (`ids.rs`'s own rule that this crate is the only place a
/// `PayrollRunId` is constructed).
pub async fn verify_payroll_run_belongs_to_employer(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    payroll_run_id: &str,
) -> Result<PayrollRunId, PayrollAppError> {
    let payroll_run_id = parse_payroll_run_id(payroll_run_id)?;
    let found: Option<bool> =
        sqlx::query_scalar("SELECT TRUE FROM payroll_run WHERE id = $1::uuid AND employer_id = $2")
            .bind(payroll_run_id.as_str())
            .bind(employer_id.as_str())
            .fetch_optional(db.pool())
            .await?;
    if found.is_some() {
        Ok(payroll_run_id)
    } else {
        Err(PayrollAppError::PayrollRunNotFound(payroll_run_id))
    }
}

/// One PayrollRun in `GET /api/employers/{e}/payroll-runs` (issue #53):
/// enough to open the right one, without its members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayrollRunSummary {
    pub id: PayrollRunId,
    pub period: PayPeriod,
    pub pay_date: NaiveDate,
    pub status: RunStatus,
}

/// Every PayrollRun `employer_id` has, in a stable order. Filters on
/// `employer_id` in SQL (ADR-0017's second layer); no pagination, no
/// filters, no sorting (issue #53's own Deep Instructions) beyond the
/// stable order below — the same discipline
/// [`crate::list_employments_for_employer`] already follows.
pub async fn list_payroll_runs(
    db: &SaltDatabase,
    employer_id: &EmployerId,
) -> Result<Vec<PayrollRunSummary>, PayrollAppError> {
    type Row = (String, NaiveDate, NaiveDate, NaiveDate, String);

    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id::text, period_start, period_end, pay_date, status
         FROM payroll_run
         WHERE employer_id = $1
         ORDER BY created_at, id",
    )
    .bind(employer_id.as_str())
    .fetch_all(db.pool())
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(id, period_start, period_end, pay_date, status)| PayrollRunSummary {
                id: PayrollRunId::new(id),
                period: PayPeriod::new(period_start, period_end)
                    .expect("payroll_run CHECK: period_end is never before period_start"),
                pay_date,
                status: RunStatus::from_column(&status),
            },
        )
        .collect())
}

/// Why `calculate_payroll_run` would refuse to pay this member right now,
/// read from standing facts rather than remembered from a failed Calculate
/// (issue #54, §0.31). Exactly the five states a fact lookup alone can
/// answer — the same five codes the error contract already owns (issue
/// #50) — and no others: a refusal that only appears once arithmetic runs
/// (inside [`crate::calculate_payroll_run`]) is out of this list's reach by
/// construction.
///
/// The two "present" variants matter as much as the two "unknown" ones:
/// known `PriorEmployment` figures are refused while their treatment is
/// unconfirmed, and a present `UnsupportedDeductionStatus` is refused as
/// firmly as an unknown one. A list showing only the unknowns would tell an
/// Operator they were ready when they were not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayrollRunBlocker {
    /// No `PriorEmployment` declaration is in force for the run's TaxYear.
    PriorEmploymentUnknown,
    /// A `PriorEmployment` declaration names figures, and Salt has no policy
    /// for how a new Employer must treat them (SC-OPEN-4).
    PriorEmploymentTreatmentUnconfirmed { figures: PriorEmploymentFigures },
    /// No `UnsupportedDeductionStatus` declaration is in force at the
    /// period end.
    UnsupportedDeductionStatusUnknown,
    /// An `UnsupportedDeductionStatus` declaration in force at the period
    /// end is `Present`. `kinds` names every kind declared, so "you have
    /// unsupported deductions" is never reported unactionably.
    UnsupportedDeductionsPresent { kinds: UnsupportedDeductionKinds },
    /// No `CompensationTerms` row is in force at the period end.
    NoCompensationTermsInForce,
}

/// The figures §0.29 names for one member's current calculation: Basic Pay,
/// Taxable Allowances, Gross, Taxable Remuneration, PAYE, Employee SSC,
/// Employer SSC, Total Deductions and Net — nine in all — read straight off
/// a stored `PayrollCalculation` (issue #55, extended to nine by issue #57
/// so a finalized read and a working one share one shape). `basic_pay` and
/// `taxable_allowances` are summed from `earning_lines` here, once, because
/// `calculate` itself never stores either as a bare total —
/// `RemunerationBases` accumulates into three statutory bases, not per-kind
/// totals. `total_deductions` is summed from `calculation.deductions`
/// itself, the same PAYE-plus-employee-SSC total `net_pay` is already
/// derived from (INV-007: employer SSC never appears in it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayrollFigures {
    pub basic_pay: Money,
    pub taxable_allowances: Money,
    pub gross: Money,
    pub taxable_remuneration: Money,
    pub paye: Money,
    pub employee_social_security: Money,
    pub employer_social_security: Money,
    pub total_deductions: Money,
    pub net_pay: Money,
}

impl PayrollFigures {
    /// Reads a stored calculation's figures straight off it — no handler
    /// upstream of this ever inspects a figure or recomputes one (issue
    /// #55's own Deep Instructions). Shared by a working run's own detail
    /// and by [`crate::get_finalized_payroll_detail`] (issue #57), so a
    /// figure read while `Calculated` and the same figure read back after
    /// `Finalized` can never silently diverge in shape.
    ///
    /// Summing `earning_lines` into `basic_pay` and `taxable_allowances`
    /// cannot overflow: `calculate` already summed the same lines into
    /// `gross_remuneration` via `checked_add` without overflowing, and
    /// every line is non-negative, so no subset of them can overflow
    /// either. Summing `deductions` into `total_deductions` cannot overflow
    /// for the same reason: `calculate` already subtracted the same two
    /// amounts from `gross_remuneration` via `checked_sub` without going
    /// negative.
    pub(crate) fn from_calculation(calculation: &PayrollCalculation) -> Self {
        let mut basic_pay = Money::ZERO;
        let mut taxable_allowances = Money::ZERO;
        for line in &calculation.earning_lines {
            match *line {
                Earning::BasicPay(amount) => {
                    basic_pay = basic_pay
                        .checked_add(amount)
                        .expect("see from_calculation's own doc comment: cannot overflow here")
                }
                Earning::TaxableAllowance(amount) => {
                    taxable_allowances = taxable_allowances
                        .checked_add(amount)
                        .expect("see from_calculation's own doc comment: cannot overflow here")
                }
            }
        }
        let total_deductions = Money::checked_sum(
            calculation
                .deductions
                .iter()
                .copied()
                .map(Deduction::amount),
        )
        .expect("see from_calculation's own doc comment: cannot overflow here");
        PayrollFigures {
            basic_pay,
            taxable_allowances,
            gross: calculation.gross_remuneration,
            taxable_remuneration: calculation.taxable_remuneration,
            paye: calculation.paye.amount,
            employee_social_security: calculation.employee_social_security.amount,
            employer_social_security: calculation.employer_social_security.amount,
            total_deductions,
            net_pay: calculation.net_pay,
        }
    }
}

/// One member of a PayrollRun in `GET
/// /api/employers/{e}/payroll-runs/{r}` (issue #53): who is being proposed
/// to pay, by name, their current Earning lines, and why they cannot be paid
/// right now, if at all (issue #54). A struct of its own — not a tuple, not
/// a bare `Vec<Earning>` beside a name — so `blockers` is an added field
/// here, not a reshaped response.
///
/// `figures` (issue #55) is read from the member's stored
/// `WorkingPayrollCalculation`, so a refresh after a successful Calculate
/// shows it again, and a member whose last calculation refused has `None`
/// — `calculate_payroll_run` clears the row rather than leaving a stale
/// one looking current.
///
/// A member's **refusal** is deliberately not a field here. A refusal is
/// "what the calculator actually said" on one call, is never persisted,
/// and so could only ever be `None` on this read — a field that can never
/// be filled is a field that lies. [`crate::calculate_payroll_run`]
/// returns its refusals named by `EmploymentId` instead, and
/// `salt-server`'s calculate handler joins them onto this detail when it
/// builds its response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayrollRunMember {
    pub employment_id: EmploymentId,
    pub full_name: String,
    pub earnings: Vec<Earning>,
    pub blockers: Vec<PayrollRunBlocker>,
    pub figures: Option<PayrollFigures>,
}

/// One PayrollRun in full, for `GET /api/employers/{e}/payroll-runs/{r}`
/// (issue #53): its period, pay date and status, and every member it
/// proposes to pay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayrollRunDetail {
    pub id: PayrollRunId,
    pub period: PayPeriod,
    pub pay_date: NaiveDate,
    pub status: RunStatus,
    pub members: Vec<PayrollRunMember>,
}

/// Reads one PayrollRun back for display, scoped to `employer_id` in SQL
/// (ADR-0017): an id belonging to another Employer is refused exactly like
/// one that does not exist at all, both as
/// [`PayrollAppError::PayrollRunNotFound`].
///
/// Members are read separately from the run's own row, not joined: a run
/// with no active members (every overlapping Employment already removed)
/// still has a period, pay date and status to show, and an inner join would
/// make that run indistinguishable from one that does not exist.
///
/// A handler never computes, adds to, or filters this membership (issue
/// #53's own Deep Instructions) — it is exactly `active_member_ids`' set,
/// read here with the name and Earning lines a display needs instead of the
/// bare ids that function returns.
pub async fn get_payroll_run_detail(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    payroll_run_id: &str,
) -> Result<PayrollRunDetail, PayrollAppError> {
    let payroll_run_id = parse_payroll_run_id(payroll_run_id)?;

    type RunRow = (NaiveDate, NaiveDate, NaiveDate, String);

    let run: Option<RunRow> = sqlx::query_as(
        "SELECT period_start, period_end, pay_date, status
         FROM payroll_run
         WHERE id = $1::uuid AND employer_id = $2",
    )
    .bind(payroll_run_id.as_str())
    .bind(employer_id.as_str())
    .fetch_optional(db.pool())
    .await?;

    let (period_start, period_end, pay_date, status) =
        run.ok_or_else(|| PayrollAppError::PayrollRunNotFound(payroll_run_id.clone()))?;
    let period = PayPeriod::new(period_start, period_end)
        .expect("payroll_run CHECK: period_end is never before period_start");

    type MemberRow = (
        String,
        String,
        Option<serde_json::Value>,
        Option<serde_json::Value>,
    );

    let member_rows: Vec<MemberRow> = sqlx::query_as(
        "SELECT employment.id, person.full_name, earnings.earning_jsons,
                working_payroll_calculation.payroll_calculation_json
         FROM payroll_run_employment
         JOIN employment
           ON employment.id = payroll_run_employment.employment_id
         JOIN person
           ON person.id = employment.person_id
          AND person.employer_id = employment.employer_id
         LEFT JOIN LATERAL (
             SELECT jsonb_agg(earning_json ORDER BY line) AS earning_jsons
             FROM payroll_run_earning
             WHERE payroll_run_earning.payroll_run_id = payroll_run_employment.payroll_run_id
               AND payroll_run_earning.employment_id = payroll_run_employment.employment_id
         ) AS earnings ON TRUE
         LEFT JOIN working_payroll_calculation
           ON working_payroll_calculation.payroll_run_id = payroll_run_employment.payroll_run_id
          AND working_payroll_calculation.employment_id = payroll_run_employment.employment_id
         WHERE payroll_run_employment.payroll_run_id = $1::uuid
           AND payroll_run_employment.removed_at IS NULL
         ORDER BY employment.id",
    )
    .bind(payroll_run_id.as_str())
    .fetch_all(db.pool())
    .await?;

    let mut members = Vec::with_capacity(member_rows.len());
    for (employment_id, full_name, earning_jsons, calculation_json) in member_rows {
        let earnings = earning_jsons
            .map(|value| {
                serde_json::from_value::<Vec<Earning>>(value)
                    .expect("payroll_run_earning.earning_json always serializes an Earning")
            })
            .unwrap_or_default();
        let figures = calculation_json.map(|value| {
            let calculation: PayrollCalculation = serde_json::from_value(value).expect(
                "working_payroll_calculation.payroll_calculation_json always serializes a \
                 PayrollCalculation",
            );
            PayrollFigures::from_calculation(&calculation)
        });
        let employment_id = EmploymentId::new(employment_id);
        let blockers = member_blockers(db, &employment_id, period).await?;
        members.push(PayrollRunMember {
            employment_id,
            full_name,
            earnings,
            blockers,
            figures,
        });
    }

    Ok(PayrollRunDetail {
        id: payroll_run_id.clone(),
        period,
        pay_date,
        status: RunStatus::from_column(&status),
        members,
    })
}

/// The `blockers` list for one active member, computed by reading exactly
/// the standing facts [`crate::calculate_payroll_run`] itself reads when it
/// assembles that member's `PayrollInput` — never by running the calculator
/// (issue #54, §0.31). An empty list means ready.
///
/// Order matches the read model's own listing: PriorEmployment, then
/// UnsupportedDeductionStatus, then CompensationTerms. Each fact yields at
/// most one blocker, so the list holds zero to three entries.
///
/// Only two other [`PayrollAppError`]s can reach the caller from here, and
/// neither is one of the five standing-fact states this list may report, so
/// both are propagated rather than swallowed into a blocker that would
/// misname them. Neither is reachable in practice:
/// [`PayrollAppError::EmploymentNotFound`] cannot happen because the
/// membership row naming this Employment is a foreign key to it, and
/// [`PayrollAppError::EmploymentIsVoid`] cannot happen because migration
/// 0018 refuses to void an Employment that is an active run member.
async fn member_blockers(
    db: &SaltDatabase,
    employment_id: &EmploymentId,
    period: PayPeriod,
) -> Result<Vec<PayrollRunBlocker>, PayrollAppError> {
    let mut blockers = Vec::new();

    let tax_year = TaxYear::for_period_end(period.end());
    match get_prior_employment_on(db.pool(), employment_id, tax_year).await? {
        PriorEmployment::None => {}
        PriorEmployment::Unknown => blockers.push(PayrollRunBlocker::PriorEmploymentUnknown),
        PriorEmployment::Some(figures) => {
            blockers.push(PayrollRunBlocker::PriorEmploymentTreatmentUnconfirmed { figures })
        }
    }

    match get_unsupported_deduction_status_on(db.pool(), employment_id, period.end()).await? {
        UnsupportedDeductionStatus::ConfirmedNone => {}
        UnsupportedDeductionStatus::Unknown => {
            blockers.push(PayrollRunBlocker::UnsupportedDeductionStatusUnknown)
        }
        UnsupportedDeductionStatus::Present(kinds) => {
            blockers.push(PayrollRunBlocker::UnsupportedDeductionsPresent { kinds })
        }
    }

    match get_employment_snapshot_on(db.pool(), employment_id, period.end()).await {
        Ok(_) => {}
        Err(PayrollAppError::NoCompensationTermsInForce(_)) => {
            blockers.push(PayrollRunBlocker::NoCompensationTermsInForce)
        }
        Err(other) => return Err(other),
    }

    Ok(blockers)
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
