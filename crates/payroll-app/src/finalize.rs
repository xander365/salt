//! `FinalizePayrollRun` (§5, §12): the one atomic act that turns a
//! `Calculated` run into immutable history. Locks the run, then for every
//! member reassembles the `PayrollInput` from current facts, re-resolves
//! the `PayrollRules` via `ruleset_for(period.end())`, and recomputes the
//! `PayrollCalculation` — the same three paths `calculate_payroll_run`
//! already runs — and refuses unless all three equal what the
//! `WorkingPayrollCalculation` stored (§5.2, ADR-0010).
//!
//! **Comparing the calculation alone is the tempting simplification, and it
//! is wrong.** A PAYE band corrected outside the range an Employee reaches,
//! or a `CompensationTerms.effective_from` fixed without touching
//! `BasicPay`, both leave the money identical while the frozen input or
//! rules would differ — and Salt would write into permanent history an
//! explanation nobody approved and no screen ever showed. All three checks
//! run in order (input, rules, calculation) and stop at the first mismatch,
//! which is enough: finalization refuses the instant any one of the three
//! disagrees, and names which.
//!
//! All included Employments finalize, or none (§5.1): every member is
//! reassembled and compared before anything is written, and the
//! `FinalizedPayroll` rows, the liveness rows, the `PayrollFinalized`
//! `ActionLog` entry and the run's status change all commit in the one
//! transaction below.
//!
//! **§5.3 step 3 branches by kind**, once every member's `employment` row is
//! locked and before anything is rebuilt. The Ordinary column refuses unless
//! the immediately preceding `PayPeriod` of the same TaxYear is resolved
//! (§7.1) for every member, converting what would otherwise be silent
//! under-withholding into a visible refusal naming the Employment and the
//! unresolved period (see [`crate::sequencing`]). The Correction column
//! checks its own period only (§7.4): a named target is re-verified as
//! reversed and matching by [`validate_correction_target`], and a `None`
//! target must have [`verify_null_lineage_is_legitimate`] prove one of
//! §4.8's two null-lineage cases actually holds. Both run here, inside the
//! transaction and under the run's own lock, because that is where §5.3
//! step 3 puts them — `add_employment_to_correction_run` checks a named
//! target too, so a caller learns at once, but that earlier answer is not
//! what the insert below relies on.
//! `replaces_finalized_payroll_id` is then copied, unconditionally, from
//! each member's own membership row into its new `FinalizedPayroll` — `None`
//! for every Ordinary member, since only a Correction's row ever carries one
//! — and the `UNIQUE` constraint on that column is what decides a race
//! between two draft Corrections naming the same target (§4.8, §9): the
//! second insert's constraint violation is read back as
//! [`PayrollAppError::CorrectionTargetAlreadyReplaced`].
//!
//! A Correction that finalizes also returns the later `PayPeriod`s already
//! finalized for its Employment, as a warning (§6.4): later periods are
//! never rewritten here, and cumulative PAYE absorbs the difference at their
//! own next calculation.
//!
//! Correctness rests on the run's own `FOR UPDATE` lock and the primary key
//! on `live_finalized_payroll`, not on the isolation level or an
//! application-side status check (§5.4): `READ COMMITTED` is sufficient.

use std::collections::HashMap;

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::calculate::{assemble_and_calculate, run_pay_lines_by_member};
use crate::correction::{validate_correction_target, verify_null_lineage_is_legitimate};
use crate::database::{SaltDatabase, is_unique_violation};
use crate::employer::pay_schedule_for_employer;
use crate::employer_particulars::employer_particulars_snapshot;
use crate::error::PayrollAppError;
use crate::ids::app_id;
use crate::payroll_run::{
    EmploymentSpan, PayrollRunId, RunKind, RunStatus, active_member_ids,
    finalized_payrolls_for_run, lock_run,
};
use crate::person_particulars::person_particulars_snapshot;
use crate::sequencing::verify_the_preceding_period_is_resolved_for_every_member;
use chrono::NaiveDate;
use payroll::{
    EmployerId, EmploymentId, PayPeriod, PayrollCalculation, PayrollInput, PayrollRules, PersonId,
    TaxYear, ruleset_for,
};

/// The shape of one `FinalizedPayroll` row's frozen snapshot (§9, §9.1) —
/// not only the three JSONB blobs any more, since issue #73: version 2 was
/// the first row shape to also carry `employer_particulars_json`,
/// `person_particulars_json` and `payslip_template_version`. It also carries
/// the labelled earning-line shape introduced by issue #74. Version 3 adds
/// overtime instructions and calculated lines, including the exact-fraction
/// `DerivedHourlyRate` in each overtime trace (issue #76).
///
/// It ships from day one and is stored on every row, because it is the field
/// a future reader branches on to render old history without constructing
/// current domain types — and there are no in-place JSON migrations, ever
/// (the application has no `UPDATE` grant on the table). A shape change means
/// this constant becomes the next integer and new rows carry it; rows
/// written at an earlier version stay there and are still read by the
/// reader written for them.
///
/// **What this integer is not for**, since issue #73's own Deep
/// Instructions: deciding whether a *particular* field froze. Version 2 rows
/// can themselves carry a null `employer_particulars_json` — an Employer
/// that simply had never recorded any at finalize time — so "are the frozen
/// particulars present" is answered by reading that column, never by
/// comparing this one against 2. This integer answers a narrower question:
/// which decoder reads the JSONB blobs (see `KNOWN_JSON_SNAPSHOT_VERSIONS`
/// and `crate::correction::prepopulate_pay_lines`).
pub const SNAPSHOT_SCHEMA_VERSION: i32 = 4;

/// The `snapshot_schema_version` values whose `payroll_input_json` and
/// `payroll_calculation_json` this build can deserialize (§9.1). Version 2
/// (issue #73) added three new sibling *columns* and issue #74 added the
/// current labelled earning-line shape, so 1 and 2 read through the same
/// compatible decoder. Version 3's new overtime variants are absent from old
/// snapshots, so the same backward-compatible decoder also reads it. Version
/// 4 (issue #78) adds `PayrollInput.deductions`, a genuinely new field absent
/// from every earlier version's frozen `payroll_input_json` — see
/// [`DEDUCTIONS_INTRODUCED_AT_SNAPSHOT_SCHEMA_VERSION`] for how
/// `crate::correction::prepopulate_pay_lines` tells that legitimate absence
/// apart from an unreadable one. This list is the one place
/// [`crate::finalized_payroll_read::calculation_from_snapshot`] and
/// `crate::correction::prepopulate_pay_lines` both check against, rather than
/// each comparing a stored version to [`SNAPSHOT_SCHEMA_VERSION`] directly —
/// a row is not unreadable merely for having finalized under an earlier
/// version whose lines this build can still decode.
pub(crate) const KNOWN_JSON_SNAPSHOT_VERSIONS: &[i32] = &[1, 2, 3, 4];

/// The first `snapshot_schema_version` whose frozen `payroll_input_json`
/// carries a `deductions` field at all (issue #78). A snapshot strictly
/// before this version has no such field by construction — reading that
/// absence as "no deductions" is a fact about history, not a guess — while a
/// snapshot at or after it that lacks the field is corrupt or from a shape
/// this build does not otherwise know, and stays unreadable rather than
/// being read as empty (§9.1).
pub(crate) const DEDUCTIONS_INTRODUCED_AT_SNAPSHOT_SCHEMA_VERSION: i32 = 4;

/// The `PayslipTemplateVersion` [`finalize_payroll_run`] freezes onto every
/// `FinalizedPayroll` (issue #73, CONTEXT.md's own glossary entry, Grill
/// Brief D12): the identifier a later ticket's renderer will be selected by,
/// chosen now because it must be frozen at the moment history is written,
/// not invented after a renderer exists to justify it.
///
/// Scheme: `"<family>-v<n>"`. `family` names a template's overall shape —
/// distinct families exist for print requirements that differ in kind (a
/// different statutory layout, say), not merely in degree. `n` bumps
/// whenever `family`'s own layout changes in a way that would render an
/// already-issued payslip differently. Both halves are stable once written:
/// a retired version's renderer is kept alive forever (Grill Brief
/// ADR-0021), because any `n` ever frozen here may be asked to re-render at
/// any time — `n` is exactly the key that future dispatch switches on.
pub const PAYSLIP_TEMPLATE_VERSION: &str = "standard-v1";

app_id! {
    /// `payroll-app`'s own id for a `FinalizedPayroll` row (§4.1): a native
    /// `uuid`, minted only here, where the row itself is inserted. The only
    /// way a caller ever holds one is by receiving it back from
    /// [`finalize_payroll_run`], the same discipline [`crate::PayrollRunId`]
    /// follows.
    FinalizedPayrollId
}

/// One member's approved `WorkingPayrollCalculation`, read back so its three
/// values can be compared against a fresh reassembly/re-resolution/recompute
/// of the same three (§5.2).
struct WorkingCalculation {
    input: PayrollInput,
    rules: PayrollRules,
    calculation: PayrollCalculation,
}

/// Finalizes `payroll_run_id`: the one atomic act that rebuilds every
/// member's figures from current facts, refuses unless they still equal
/// what was approved, and — only then — writes the immutable
/// `FinalizedPayroll` history (§5).
///
/// Refused outright when the run does not exist, is already `Finalized`, or
/// is not yet `Calculated` (§4.7) — finalizing is only ever a move out of
/// `Calculated`.
///
/// A run with no active members finalizes vacuously, for the same reason
/// `calculate_payroll_run` lets one become `Calculated`: §4.7 defines the
/// state as a property of the members, and an Employer who removed everyone
/// with a stated reason has said something complete about the period. A
/// Correction run holds at most one member (§4.8), so this is the same case
/// there, not a special one.
///
/// Returns every member's newly-minted [`FinalizedPayrollId`] alongside its
/// `EmploymentId`, empty for a vacuous run, plus — for a Correction —
/// every later `PayPeriod` already finalized for its Employment, as a
/// warning (§6.4); always empty for an Ordinary run.
pub async fn finalize_payroll_run(
    db: &SaltDatabase,
    payroll_run_id: &PayrollRunId,
    finalized_by: &str,
) -> Result<FinalizationOutcome, PayrollAppError> {
    let mut tx = db.pool().begin().await?;

    // The `FOR UPDATE` lock taken here is what serialises two finalizers of
    // the same run (§5.4): the loser blocks until the winner commits, then
    // re-reads `status` as `finalized` and refuses below — no
    // application-side `if status != Finalized` is doing that work.
    let run = lock_run(&mut tx, payroll_run_id).await?;
    if run.status == RunStatus::Finalized {
        let finalized_payrolls =
            finalized_payrolls_for_run(&mut tx, payroll_run_id, run.period.end()).await?;
        return Err(PayrollAppError::PayrollRunAlreadyFinalized {
            payroll_run_id: payroll_run_id.clone(),
            finalized_payrolls,
        });
    }
    if run.status != RunStatus::Calculated {
        return Err(PayrollAppError::PayrollRunNotCalculated(
            payroll_run_id.clone(),
        ));
    }
    // The period needs no separate verification: it is read from the locked
    // run itself and every rebuild below resolves rules, year-to-date and
    // the snapshot from that one value, so there is no second period for it
    // to disagree with.
    let period = run.period;
    let employer_id = run.employer_id.clone();

    let schedule = pay_schedule_for_employer(&mut tx, &employer_id).await?;

    // The frozen `EmployerParticulars` snapshot every member's
    // `FinalizedPayroll` carries (issue #73): read once, here, because it
    // does not vary by member, and the `FOR SHARE` `pay_schedule_for_employer`
    // just took on the `employer` row above is what already makes this read
    // hold against a concurrent `set_employer_particulars` (`freeze.rs`'s
    // module doc) — no lock of this function's own.
    let employer_particulars_json = employer_particulars_snapshot(&mut tx, &employer_id).await?;

    // Resolved once, outside the per-member loop, for the same reason
    // `calculate_payroll_run` resolves it once: it depends only on the
    // period, not on any one member.
    let rules = ruleset_for(period.end())?;
    let tax_year = TaxYear::for_period_end(period.end());

    let member_ids = active_member_ids(&mut tx, payroll_run_id).await?;
    // A Correction run holds at most one member (§4.8), and both the
    // null-lineage check below and the later-periods warning after it are
    // about that one Employment. Read once, here, so the two never disagree
    // about which member they mean. `None` for an Ordinary run, whatever its
    // membership, and for a vacuous Correction.
    let corrected_employment_id = match run.kind {
        RunKind::Correction => member_ids.first().cloned().map(EmploymentId::new),
        RunKind::Ordinary => None,
    };

    // Every member's `employment` row is locked *before* any master data is
    // read (ADR-0013): `record_opening_balance` and `declare_prior_employment`
    // take `FOR UPDATE` on the same row, and `FOR SHARE` conflicts with it,
    // so an edit of a frozen fact and this finalization can never interleave.
    // Whichever arrives second waits: an edit that loses the race sees this
    // transaction's `FinalizedPayroll` and refuses; a finalization that loses
    // re-reads the edited fact below and refuses with a mismatch rather than
    // finalizing figures nobody approved. `ORDER BY id` fixes one acquisition
    // order for every finalizer, so two overlapping runs queue rather than
    // deadlock.
    let members = lock_member_employments(&mut tx, &member_ids).await?;

    // Every locked member's own `PersonId` (issue #73): read off the same
    // locked rows above rather than a second query, and kept as a map so
    // the write loop below can look one up per member without caring what
    // order `ready` puts them in.
    let person_id_by_employment: HashMap<String, PersonId> = members
        .iter()
        .map(|member| (member.id.clone(), PersonId::new(member.person_id.clone())))
        .collect();

    // §4.8: `None` for every Ordinary member, always — only a Correction
    // run's own single membership row ever carries a declared target. Read
    // unconditionally, by kind, rather than skipped for Ordinary: the value
    // is needed again below, to copy into each member's `FinalizedPayroll`.
    let lineage_by_member = declared_lineage_by_member(&mut tx, payroll_run_id).await?;

    // §5.3 step 3, run after the member lock above for the same reason
    // master data is read after it: a concurrent `OpeningBalance`,
    // `PriorEmployment` or run-membership write on these same `employment`
    // rows must not interleave with this read (§5.4).
    match run.kind {
        RunKind::Ordinary => {
            // §7.2: an Ordinary run refuses unless the immediately preceding
            // PayPeriod of the same TaxYear is resolved for every member. It
            // is handed the spans the lock itself already read, rather than
            // reading those same locked rows a second time.
            verify_the_preceding_period_is_resolved_for_every_member(
                &mut tx,
                &employer_id,
                schedule,
                period,
                tax_year,
                &members,
            )
            .await?;
        }
        RunKind::Correction => {
            // §7.4: a Correction run checks its own period only — no walk
            // back, no walk forward. §5.3 step 3 puts both halves of the
            // check here, inside the transaction, under the run's own
            // `FOR UPDATE` lock: a named target must (still) be reversed and
            // matching, and a `None` target must itself prove one of §4.8's
            // two null-lineage cases. `add_employment_to_correction_run`
            // checks a named target too, so a caller learns at once, but
            // that answer is not what this insert relies on.
            if let Some(employment_id) = &corrected_employment_id {
                match lineage_by_member.get(employment_id).cloned().flatten() {
                    Some(target) => {
                        validate_correction_target(
                            &mut tx,
                            payroll_run_id,
                            employment_id,
                            &run,
                            &target,
                        )
                        .await?;
                    }
                    None => {
                        verify_null_lineage_is_legitimate(
                            &mut tx,
                            &employer_id,
                            employment_id,
                            period,
                        )
                        .await?;
                    }
                }
            }
        }
    }

    let mut earnings_by_member = run_pay_lines_by_member(&mut tx, payroll_run_id).await?;

    // Every member is reassembled and compared before anything is written
    // (§5.1): a mismatch on the last member must leave every earlier member
    // with no `FinalizedPayroll` row either.
    let mut ready = Vec::with_capacity(member_ids.len());
    for member_id in member_ids {
        let employment_id = EmploymentId::new(member_id.clone());
        let stored = fetch_working_calculation(&mut tx, payroll_run_id, &employment_id).await?;
        let earnings = earnings_by_member.remove(&member_id).unwrap_or_default();

        // A rebuild that refuses outright — a `CompensationTerms` row deleted,
        // an Employment voided, a declaration withdrawn since the run
        // calculated — is named by the Employment it blocked, for the same
        // reason the three mismatches below are and `calculate_payroll_run`
        // returns its refusals as `PayrollRunCalculationRefusal`: an Employer
        // told only "PriorEmployment is Unknown" about a ten-member run has
        // been told nothing they can act on.
        let (current_input, current_calculation) =
            assemble_and_calculate(&mut tx, &employment_id, period, schedule, earnings, &rules)
                .await
                .map_err(|refusal| PayrollAppError::FinalizationRebuildRefused {
                    employment_id: employment_id.clone(),
                    refusal: Box::new(refusal),
                })?;

        if current_input != stored.input {
            return Err(PayrollAppError::FinalizationInputMismatch {
                employment_id,
                approved: Box::new(stored.input),
                current: Box::new(current_input),
            });
        }
        if rules != stored.rules {
            return Err(PayrollAppError::FinalizationRulesMismatch {
                employment_id,
                approved: Box::new(stored.rules),
                current: Box::new(rules),
            });
        }
        if current_calculation != stored.calculation {
            return Err(PayrollAppError::FinalizationCalculationMismatch {
                employment_id,
                approved: Box::new(stored.calculation),
                current: Box::new(current_calculation),
            });
        }

        let replaces_finalized_payroll_id =
            lineage_by_member.get(&employment_id).cloned().flatten();
        ready.push((
            employment_id,
            current_input,
            current_calculation,
            replaces_finalized_payroll_id,
        ));
    }

    let mut finalized_ids = Vec::with_capacity(ready.len());
    for (employment_id, input, calculation, replaces_finalized_payroll_id) in ready {
        let person_id = person_id_by_employment.get(employment_id.as_str()).expect(
            "every ready member's employment was locked above, which is where this map came from",
        );
        // The frozen `PersonParticulars` snapshot (issue #73): fetched per
        // member, here, because it is Person-scoped rather than
        // Employer-scoped — the `FOR SHARE` `lock_member_employments` took
        // on this member's own `employment` row is what makes this read hold
        // against a concurrent `set_person_particulars`/
        // `correct_person_full_name` (`freeze.rs`'s module doc).
        let person_particulars_json = person_particulars_snapshot(&mut tx, person_id).await?;

        let finalized_payroll_id = insert_finalized_payroll(
            &mut tx,
            payroll_run_id,
            &employer_id,
            &employment_id,
            period,
            tax_year,
            &input,
            &rules,
            &calculation,
            replaces_finalized_payroll_id
                .as_ref()
                .map(FinalizedPayrollId::as_str),
            employer_particulars_json.clone(),
            person_particulars_json,
            PAYSLIP_TEMPLATE_VERSION,
            finalized_by,
        )
        .await?;

        // The concurrency guard (§5.4, §6.2): a second live row for this
        // `(employment_id, period_end)` is unrepresentable, whatever the
        // `FOR UPDATE` lock above believed — the run lock serialises two
        // finalizers of one run, and this key serialises two runs that would
        // pay one Employment for one period.
        //
        // Its violation is read back as a domain refusal for the same reason
        // the `replaces_finalized_payroll_id` UNIQUE violation is: the two
        // guard the same act from the two sides §4.8 names, and a caller who
        // lost one race must not be told "duplicate key" while a caller who
        // lost the other is told what to do next.
        let live_insert = sqlx::query(
            "INSERT INTO live_finalized_payroll (employment_id, period_end, finalized_payroll_id)
             VALUES ($1, $2, $3::uuid)",
        )
        .bind(employment_id.as_str())
        .bind(period.end())
        .bind(finalized_payroll_id.as_str())
        .execute(&mut *tx)
        .await;
        if let Err(err) = live_insert {
            if is_unique_violation(&err, ONE_LIVE_PAYROLL_PER_EMPLOYMENT_AND_PERIOD) {
                return Err(PayrollAppError::CorrectionPeriodAlreadyHasALivePayroll {
                    employment_id: employment_id.clone(),
                    period,
                });
            }
            return Err(err.into());
        }

        finalized_ids.push((employment_id, finalized_payroll_id));
    }

    write_action_log_entry(
        &mut tx,
        ActionLogEntry {
            employer_id: &employer_id,
            actor: finalized_by,
            action_type: ActionType::PayrollFinalized,
            target_type: "payroll_run",
            target_id: payroll_run_id.as_str(),
            context: None,
        },
    )
    .await?;

    sqlx::query("UPDATE payroll_run SET status = 'finalized' WHERE id = $1::uuid")
        .bind(payroll_run_id.as_str())
        .execute(&mut *tx)
        .await?;

    // §6.4: a warning, not a refusal, and only ever for a Correction — later
    // periods are never rewritten (§7.4), so their own finalized figures are
    // untouched by this transaction. Read after everything else succeeds, on
    // the same connection, so it is a consistent account of what this
    // finalization actually left behind.
    let later_finalized_periods = match &corrected_employment_id {
        Some(employment_id) => {
            later_finalized_periods_for(&mut tx, employment_id, period.end()).await?
        }
        None => Vec::new(),
    };

    tx.commit().await?;
    Ok(FinalizationOutcome {
        finalized: finalized_ids,
        later_finalized_periods,
    })
}

/// Every already-finalized `PayPeriod` strictly after `period_end`, live,
/// for `employment_id` — the list a Correction's caller acts on (§6.4):
/// later periods are never rewritten, and cumulative PAYE absorbs the
/// difference at their own next calculation.
///
/// Not narrowed to the corrected period's own TaxYear. §6.4 names "the later
/// finalized periods" without qualification, and the caller is the one
/// placed to judge: cumulative PAYE does restart each TaxYear (ADR-0001), so
/// a period in the next one usually needs nothing — but "usually" is a
/// judgement, and silently dropping the rows takes it away from them.
async fn later_finalized_periods_for(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    employment_id: &EmploymentId,
    period_end: NaiveDate,
) -> Result<Vec<PayPeriod>, PayrollAppError> {
    let rows: Vec<(NaiveDate, NaiveDate)> = sqlx::query_as(
        "SELECT finalized.period_start, finalized.period_end
         FROM live_finalized_payroll AS live
         JOIN finalized_payroll AS finalized ON finalized.id = live.finalized_payroll_id
         WHERE live.employment_id = $1 AND live.period_end > $2
         ORDER BY live.period_end",
    )
    .bind(employment_id.as_str())
    .bind(period_end)
    .fetch_all(&mut **tx)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(start, end)| {
            PayPeriod::new(start, end)
                .expect("finalized_payroll CHECK: period_end is never before period_start")
        })
        .collect())
}

/// The declared `replaces_finalized_payroll_id` of every active member of
/// `payroll_run_id` (§4.8). Always `None` for an Ordinary member — only a
/// Correction run's single membership row ever carries one — so
/// [`finalize_payroll_run`] reads it unconditionally, for every kind, rather
/// than branching to skip it.
///
/// Lives here rather than beside the other membership reads in
/// `payroll_run.rs` because [`FinalizedPayrollId`] is minted here, and
/// finalization is its only reader.
async fn declared_lineage_by_member(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payroll_run_id: &PayrollRunId,
) -> Result<HashMap<EmploymentId, Option<FinalizedPayrollId>>, PayrollAppError> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT employment_id, replaces_finalized_payroll_id::text
         FROM payroll_run_employment
         WHERE payroll_run_id = $1::uuid AND removed_at IS NULL",
    )
    .bind(payroll_run_id.as_str())
    .fetch_all(&mut **tx)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(employment_id, target)| {
            (
                EmploymentId::new(employment_id),
                target.map(FinalizedPayrollId::new),
            )
        })
        .collect())
}

/// What [`finalize_payroll_run`] returns on success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizationOutcome {
    /// Every member's newly-minted [`FinalizedPayrollId`] alongside its
    /// `EmploymentId`, empty for a vacuous run.
    pub finalized: Vec<(EmploymentId, FinalizedPayrollId)>,
    /// For a Correction, every later `PayPeriod` already finalized for its
    /// Employment (§6.4) — always empty for an Ordinary run, and empty for a
    /// vacuous Correction with no member.
    pub later_finalized_periods: Vec<PayPeriod>,
}

/// Takes `FOR SHARE` on each member's `employment` row, in one statement and
/// in a fixed order, and returns the span each locked row carries. Reading
/// the rows back is what proves the lock was taken: a member whose
/// Employment vanished between the membership read and this one is
/// impossible — `employment` is never physically deleted (§4.3) — so a short
/// result would mean the schema no longer matches this code.
///
/// The spans come back rather than being discarded because the caller's very
/// next step needs them (§7.1 branch 1), and re-reading a row this statement
/// is already holding would be a second read of the same locked fact.
async fn lock_member_employments(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    member_ids: &[String],
) -> Result<Vec<EmploymentSpan>, PayrollAppError> {
    let locked: Vec<EmploymentSpan> = sqlx::query_as(
        "SELECT id, person_id, start_date, end_date FROM employment
         WHERE id = ANY($1) ORDER BY id FOR SHARE",
    )
    .bind(member_ids)
    .fetch_all(&mut **tx)
    .await?;

    assert_eq!(
        locked.len(),
        member_ids.len(),
        "an Employment is never physically deleted (§4.3), so every active \
         member of a run still has a row to lock"
    );
    Ok(locked)
}

/// Reads back one member's approved `WorkingPayrollCalculation`. A missing
/// row here would mean the run's own `Calculated` status lied — every active
/// member has one by definition (§4.7) — and the run lock held since before
/// this function was called rules out a membership or calculation change
/// slipping in underneath it.
async fn fetch_working_calculation(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payroll_run_id: &PayrollRunId,
    employment_id: &EmploymentId,
) -> Result<WorkingCalculation, PayrollAppError> {
    let (input_json, rules_json, calculation_json): (
        serde_json::Value,
        serde_json::Value,
        serde_json::Value,
    ) = sqlx::query_as(
        "SELECT payroll_input_json, payroll_rules_json, payroll_calculation_json
         FROM working_payroll_calculation
         WHERE payroll_run_id = $1::uuid AND employment_id = $2",
    )
    .bind(payroll_run_id.as_str())
    .bind(employment_id.as_str())
    .fetch_one(&mut **tx)
    .await?;

    Ok(WorkingCalculation {
        input: serde_json::from_value(input_json).expect(
            "working_payroll_calculation.payroll_input_json is always a serialized PayrollInput",
        ),
        rules: serde_json::from_value(rules_json).expect(
            "working_payroll_calculation.payroll_rules_json is always a serialized PayrollRules",
        ),
        calculation: serde_json::from_value(calculation_json).expect(
            "working_payroll_calculation.payroll_calculation_json is always a serialized PayrollCalculation",
        ),
    })
}

/// Inserts one immutable `FinalizedPayroll` row, freezing the complete
/// `PayrollInput`, `PayrollRules` and `PayrollCalculation`, both rule ids,
/// the `SaltVersion`, and `TaxableRemuneration`/`PAYE` as real numeric
/// columns beside the JSONB snapshots (§9) — the columns year-to-date
/// actually reads.
///
/// `snapshot_schema_version` is bound explicitly from
/// [`SNAPSHOT_SCHEMA_VERSION`] rather than left to the column's `DEFAULT 1`
/// (migration 0009). §9.1 makes it the field a future reader branches on to
/// render old history, and there are no in-place JSON migrations ever — so
/// the version must be the one *this code's* snapshot shape actually is. A
/// default states what the column was created with, which stops being the
/// same fact the day a shape change ships.
///
/// `replaces_finalized_payroll_id` is `None` for every Ordinary member and,
/// for a Correction's one member, whatever it declared (§4.8) — copied
/// through unconditionally by the caller, never decided here. A `UNIQUE`
/// violation on it — another Correction finalizing against the same target
/// first — is read back as [`PayrollAppError::CorrectionTargetAlreadyReplaced`]
/// rather than surfacing as a raw database error (§4.8, §9).
///
/// `employer_particulars_json`, `person_particulars_json` and
/// `payslip_template_version` are issue #73's own three columns:
/// `employer_particulars_json` is whatever the caller's own
/// `employer_particulars_snapshot` read (`None` when nothing was on
/// record), `person_particulars_json` is this member's own
/// `person_particulars_snapshot` (never `None` — see that function's own
/// docs), and `payslip_template_version` is always
/// [`PAYSLIP_TEMPLATE_VERSION`] — none of the three is decided in here, all
/// three are simply frozen.
#[allow(clippy::too_many_arguments)]
async fn insert_finalized_payroll(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payroll_run_id: &PayrollRunId,
    employer_id: &EmployerId,
    employment_id: &EmploymentId,
    period: PayPeriod,
    tax_year: TaxYear,
    input: &PayrollInput,
    rules: &PayrollRules,
    calculation: &PayrollCalculation,
    replaces_finalized_payroll_id: Option<&str>,
    employer_particulars_json: Option<serde_json::Value>,
    person_particulars_json: serde_json::Value,
    payslip_template_version: &str,
    finalized_by: &str,
) -> Result<FinalizedPayrollId, PayrollAppError> {
    let input_json = serde_json::to_value(input).expect("PayrollInput always serializes");
    let rules_json = serde_json::to_value(rules).expect("PayrollRules always serializes");
    let calculation_json =
        serde_json::to_value(calculation).expect("PayrollCalculation always serializes");

    let inserted: Result<String, sqlx::Error> = sqlx::query_scalar(
        "INSERT INTO finalized_payroll
            (payroll_run_id, employment_id, employer_id, period_start, period_end, tax_year,
             replaces_finalized_payroll_id,
             payroll_input_json, payroll_rules_json, payroll_calculation_json,
             taxable_remuneration, paye, paye_table_id, ssc_rules_id, salt_version,
             snapshot_schema_version, finalized_by,
             employer_particulars_json, person_particulars_json, payslip_template_version)
         VALUES ($1::uuid, $2, $3, $4, $5, $6, $7::uuid, $8, $9, $10, $11, $12, $13, $14, $15,
                 $16, $17, $18, $19, $20)
         RETURNING id::text",
    )
    .bind(payroll_run_id.as_str())
    .bind(employment_id.as_str())
    .bind(employer_id.as_str())
    .bind(period.start())
    .bind(period.end())
    .bind(tax_year.starting_year())
    .bind(replaces_finalized_payroll_id)
    .bind(input_json)
    .bind(rules_json)
    .bind(calculation_json)
    .bind(calculation.taxable_remuneration.cents())
    .bind(calculation.paye.amount.cents())
    .bind(calculation.paye_table_id.as_str())
    .bind(calculation.ssc_rules_id.as_str())
    .bind(crate::SALT_VERSION)
    .bind(SNAPSHOT_SCHEMA_VERSION)
    .bind(finalized_by)
    .bind(employer_particulars_json)
    .bind(person_particulars_json)
    .bind(payslip_template_version)
    .fetch_one(&mut **tx)
    .await;

    let id = match inserted {
        Ok(id) => id,
        Err(err) => {
            if let Some(target) = replaces_finalized_payroll_id
                && is_unique_violation(&err, CORRECTION_TARGET_REPLACED_AT_MOST_ONCE)
            {
                return Err(PayrollAppError::CorrectionTargetAlreadyReplaced(
                    FinalizedPayrollId::new(target.to_string()),
                ));
            }
            return Err(err.into());
        }
    };
    Ok(FinalizedPayrollId::new(id))
}

/// The name PostgreSQL gives migration 0009's `UNIQUE
/// (replaces_finalized_payroll_id)` on `finalized_payroll` — the constraint
/// that makes "a reversed FinalizedPayroll is replaced at most once" true
/// (§4.8), and decides the race between two draft Corrections naming the
/// same target.
const CORRECTION_TARGET_REPLACED_AT_MOST_ONCE: &str =
    "finalized_payroll_replaces_finalized_payroll_id_key";

/// The name PostgreSQL gives migration 0010's primary key on
/// `live_finalized_payroll` — the constraint that makes "one live
/// FinalizedPayroll per Employment and PayPeriod" true (§6.2), and decides
/// between two runs that would both pay one Employment for one period.
const ONE_LIVE_PAYROLL_PER_EMPLOYMENT_AND_PERIOD: &str = "live_finalized_payroll_pkey";
