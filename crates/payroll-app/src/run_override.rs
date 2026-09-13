//! `OverrideStandingPayLine`, `RemoveStandingPayLine` and
//! `RefreshStandingProposals` (issue #80, parent #70 §0, §D-6): a run may
//! change or remove a proposed `StandingPayItem` line for that run only,
//! with a reason, leaving the standing record untouched, and the operator
//! decides when a draft's proposals catch up with what the standing records
//! say now.
//!
//! **A draft never refreshes silently.** [`diff_standing_proposal`] is the
//! pure comparison every read of a run's detail runs to report the
//! difference; [`refresh_standing_proposals`] is the separate, explicit act
//! that actually writes it. Neither function ever re-proposes from scratch —
//! both key strictly on `standing_pay_item_id`, and a line carrying a
//! non-blank `override_reason` or `removed_reason` is untouched by either.
//! That keying is the whole mechanism by which deliberate work survives:
//! there is no second bookkeeping list.

use chrono::NaiveDate;
use payroll::{EmploymentId, Money};

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::database::SaltDatabase;
use crate::error::PayrollAppError;
use crate::payroll_run::{
    PayLineInstruction, PayLineSource, PayrollRunId, RunKind, RunPayLine, active_member_ids,
    lock_editable_run, read_member_pay_lines, verify_is_active_member, write_member_pay_lines,
};
use crate::standing_pay_item::{StandingPayItemId, StandingPayItemInstruction};

/// One `StandingPayItem` as the change signal and refresh report both name
/// it: its id, its own current instruction (never an override), and when it
/// began.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandingItemProposal {
    pub standing_pay_item_id: StandingPayItemId,
    pub instruction: StandingPayItemInstruction,
    pub effective_from: NaiveDate,
}

/// How a member's standing items in force now differ from what a draft's
/// stored pay lines propose (§0, §D-6). Keyed only on `standing_pay_item_id`
/// — never re-derived from an instruction comparison — so a deliberately
/// overridden line is never mistaken for a changed one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StandingItemsChangedSinceProposal {
    /// A `StandingPayItem` in force now with no line at all for it yet — a
    /// removed line still counts as "having a line", so ending and
    /// re-recording the same item, or an operator's own removal, is never
    /// reported as an addition.
    pub added: Vec<StandingItemProposal>,
    /// A plain, active, non-overridden line whose own `instruction` no
    /// longer equals its item's. Migration 0040 makes a `StandingPayItem`
    /// immutable except for ending it, so this is unreachable through any
    /// write this build makes today; it is computed anyway; see
    /// `algorithm_a_plain_line_whose_instruction_differs_is_changed` below.
    pub changed: Vec<StandingItemProposal>,
    /// An active (not removed) line whose item is no longer in force —
    /// ended, most likely. Reported, never deleted (§0): the line stays
    /// exactly where it is, and the fact that its item ended is a fact for
    /// the operator to see and act on, not this module's to silently apply.
    pub ended: Vec<StandingItemProposal>,
}

impl StandingItemsChangedSinceProposal {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.ended.is_empty()
    }
}

/// Converts one `StandingPayItem`'s own instruction into the
/// [`PayLineInstruction`] shape a pay line stores, going through the same
/// JSON round trip [`StandingPayItemInstruction::from_pay_line_json`]
/// reads back — the one conversion this module needs the other direction
/// from `into_pay_line_instruction`.
fn to_standing_instruction(instruction: PayLineInstruction) -> StandingPayItemInstruction {
    let value = serde_json::to_value(&instruction).expect("PayLineInstruction always serializes");
    StandingPayItemInstruction::from_pay_line_json(value)
}

/// The pure comparison behind both the run detail's change signal and
/// [`refresh_standing_proposals`]'s own report: what would change if this
/// member's proposals were refreshed right now, without changing anything.
///
/// `lines` is one member's complete, currently stored pay lines (in any
/// order); `in_force` is every `StandingPayItem` in force on that member as
/// of the run's period end, read fresh. Both directions are walked once
/// each: every `in_force` item decides whether it is `added` or possibly
/// `changed`, and every stored standing line decides whether it is `ended`.
pub(crate) fn diff_standing_proposal(
    lines: &[RunPayLine],
    in_force: &[StandingItemProposal],
) -> StandingItemsChangedSinceProposal {
    let mut result = StandingItemsChangedSinceProposal::default();

    for item in in_force {
        let existing_line = lines
            .iter()
            .find(|line| line.standing_pay_item_id.as_ref() == Some(&item.standing_pay_item_id));
        match existing_line {
            None => result.added.push(item.clone()),
            // An overridden or removed line differs from its item on
            // purpose, or contributes nothing at all — neither is ever
            // `changed`.
            Some(line) if line.override_reason.is_some() || line.is_removed() => {}
            Some(line) => {
                let items_own_pay_line = item.instruction.clone().into_pay_line_instruction();
                if line.instruction != items_own_pay_line {
                    result.changed.push(item.clone());
                }
            }
        }
    }

    for line in lines {
        if line.source != PayLineSource::Standing || line.is_removed() {
            continue;
        }
        let standing_pay_item_id = line
            .standing_pay_item_id
            .as_ref()
            .expect("a Standing-sourced line always carries a StandingPayItemId");
        let still_in_force = in_force
            .iter()
            .any(|item| &item.standing_pay_item_id == standing_pay_item_id);
        if !still_in_force {
            let instruction = line
                .standing_instruction
                .clone()
                .expect("a Standing-sourced line always carries its item's own instruction");
            result.ended.push(StandingItemProposal {
                standing_pay_item_id: standing_pay_item_id.clone(),
                instruction: to_standing_instruction(instruction),
                effective_from: line
                    .standing_effective_from
                    .expect("a Standing-sourced line always carries its effective-from date"),
            });
        }
    }

    result
}

/// Whether the new instruction and the item's own instruction are the same
/// *kind* — allowance stays allowance, premium stays premium (§0). Standing
/// items have exactly two kinds, each tagged by a different
/// [`PayLineInstruction`] variant (`Earning`/`Deduction`), so comparing that
/// discriminant alone is the whole check.
fn same_pay_line_kind(a: &PayLineInstruction, b: &PayLineInstruction) -> bool {
    matches!(
        (a, b),
        (
            PayLineInstruction::Earning(_),
            PayLineInstruction::Earning(_)
        ) | (
            PayLineInstruction::Deduction(_),
            PayLineInstruction::Deduction(_)
        )
    )
}

/// Changes a proposed standing line for `payroll_run_id`'s
/// `employment_id` — for this run only, with a reason — leaving the
/// `StandingPayItem` itself untouched (§0). Overriding an already-overridden
/// line replaces the override; overriding back to exactly the item's own
/// instruction clears it (`override_reason` becomes `None`), which is the
/// "undo override" path.
///
/// Refuses, in order: a blank `reason`; a `MedicalAidPremium` override of
/// zero (the UI tells the operator to remove the line instead); a
/// `standing_pay_item_id` that is not even a well-formed UUID; the run not
/// editable ([`PayrollAppError::PayrollRunAlreadyFinalized`]); the
/// Employment not an active member; no line naming that `StandingPayItem`
/// on this member ([`PayrollAppError::StandingPayLineNotFound`]); that line
/// already removed ([`PayrollAppError::StandingPayLineIsRemoved`]); the new
/// instruction's kind not matching the item's own
/// ([`PayrollAppError::OverrideChangesPayLineKind`]).
///
/// Runs under the run's existing lock, in one transaction, through
/// `write_member_pay_lines` — the one write path every
/// pay-line change shares (§D-6's own "no second bookkeeping list"). Writes
/// an `ActionLog` entry only when something actually changed.
pub async fn override_standing_pay_line(
    db: &SaltDatabase,
    payroll_run_id: &PayrollRunId,
    employment_id: &EmploymentId,
    standing_pay_item_id: &str,
    instruction: StandingPayItemInstruction,
    reason: &str,
    actor: &str,
) -> Result<(), PayrollAppError> {
    if reason.trim().is_empty() {
        return Err(PayrollAppError::OverrideReasonCannotBeEmpty);
    }
    if instruction == StandingPayItemInstruction::MedicalAidPremium(Money::ZERO) {
        return Err(PayrollAppError::StandingMedicalAidPremiumIsZero);
    }
    if uuid::Uuid::parse_str(standing_pay_item_id).is_err() {
        return Err(PayrollAppError::StandingPayLineNotFound {
            payroll_run_id: payroll_run_id.clone(),
            employment_id: employment_id.clone(),
            standing_pay_item_id: StandingPayItemId::new(standing_pay_item_id),
        });
    }
    let standing_pay_item_id = StandingPayItemId::new(standing_pay_item_id);

    let mut tx = db.pool().begin().await?;
    let run = lock_editable_run(&mut tx, payroll_run_id).await?;
    verify_is_active_member(&mut tx, payroll_run_id, employment_id).await?;

    let stored = read_member_pay_lines(&mut tx, payroll_run_id, employment_id).await?;
    let Some(existing_index) = stored
        .iter()
        .position(|line| line.standing_pay_item_id.as_ref() == Some(&standing_pay_item_id))
    else {
        return Err(PayrollAppError::StandingPayLineNotFound {
            payroll_run_id: payroll_run_id.clone(),
            employment_id: employment_id.clone(),
            standing_pay_item_id,
        });
    };
    if stored[existing_index].is_removed() {
        return Err(PayrollAppError::StandingPayLineIsRemoved {
            payroll_run_id: payroll_run_id.clone(),
            employment_id: employment_id.clone(),
            standing_pay_item_id,
        });
    }

    let standing_instruction = stored[existing_index]
        .standing_instruction
        .clone()
        .expect("a Standing-sourced line always carries its item's own instruction");
    let new_instruction = instruction.into_pay_line_instruction();
    if !same_pay_line_kind(&standing_instruction, &new_instruction) {
        return Err(PayrollAppError::OverrideChangesPayLineKind {
            payroll_run_id: payroll_run_id.clone(),
            employment_id: employment_id.clone(),
            standing_pay_item_id,
        });
    }

    let override_reason = if new_instruction == standing_instruction {
        None
    } else {
        Some(reason.trim().to_owned())
    };

    let before = stored[existing_index].instruction.clone();
    let mut new_lines = stored.clone();
    new_lines[existing_index] = RunPayLine {
        instruction: new_instruction.clone(),
        source: PayLineSource::Standing,
        standing_pay_item_id: Some(standing_pay_item_id.clone()),
        standing_effective_from: stored[existing_index].standing_effective_from,
        override_reason,
        removed_reason: None,
        standing_instruction: Some(standing_instruction),
    };

    let wrote =
        write_member_pay_lines(&mut tx, payroll_run_id, employment_id, &stored, new_lines).await?;
    if wrote {
        write_action_log_entry(
            &mut tx,
            ActionLogEntry {
                employer_id: &run.employer_id,
                actor,
                action_type: ActionType::StandingPayLineOverridden,
                target_type: "employment",
                target_id: employment_id.as_str(),
                context: Some(serde_json::json!({
                    "payroll_run_id": payroll_run_id.as_str(),
                    "standing_pay_item_id": standing_pay_item_id.as_str(),
                    "before": before,
                    "after": new_instruction,
                    "reason": reason,
                })),
            },
        )
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// Removes a proposed standing line for `payroll_run_id`'s `employment_id`
/// — for this run only, with a reason. The line contributes nothing to
/// calculation from this point on, but stays visible with its reason (§0);
/// there is no "undo removal" in this issue (decision 6) — an operator types
/// a one-off line instead if that turns out to be wanted after all.
///
/// Refuses, in order: a blank `reason`; a `standing_pay_item_id` that is not
/// a well-formed UUID; the run not editable; the Employment not an active
/// member; no line naming that `StandingPayItem`
/// ([`PayrollAppError::StandingPayLineNotFound`]); that line already removed
/// ([`PayrollAppError::StandingPayLineAlreadyRemoved`]).
///
/// `override_reason` is left exactly as it is: a line removed after being
/// overridden keeps both facts, since both happened.
pub async fn remove_standing_pay_line(
    db: &SaltDatabase,
    payroll_run_id: &PayrollRunId,
    employment_id: &EmploymentId,
    standing_pay_item_id: &str,
    reason: &str,
    actor: &str,
) -> Result<(), PayrollAppError> {
    if reason.trim().is_empty() {
        return Err(PayrollAppError::PayLineRemovalReasonCannotBeEmpty);
    }
    if uuid::Uuid::parse_str(standing_pay_item_id).is_err() {
        return Err(PayrollAppError::StandingPayLineNotFound {
            payroll_run_id: payroll_run_id.clone(),
            employment_id: employment_id.clone(),
            standing_pay_item_id: StandingPayItemId::new(standing_pay_item_id),
        });
    }
    let standing_pay_item_id = StandingPayItemId::new(standing_pay_item_id);

    let mut tx = db.pool().begin().await?;
    let run = lock_editable_run(&mut tx, payroll_run_id).await?;
    verify_is_active_member(&mut tx, payroll_run_id, employment_id).await?;

    let stored = read_member_pay_lines(&mut tx, payroll_run_id, employment_id).await?;
    let Some(existing_index) = stored
        .iter()
        .position(|line| line.standing_pay_item_id.as_ref() == Some(&standing_pay_item_id))
    else {
        return Err(PayrollAppError::StandingPayLineNotFound {
            payroll_run_id: payroll_run_id.clone(),
            employment_id: employment_id.clone(),
            standing_pay_item_id,
        });
    };
    if stored[existing_index].is_removed() {
        return Err(PayrollAppError::StandingPayLineAlreadyRemoved {
            payroll_run_id: payroll_run_id.clone(),
            employment_id: employment_id.clone(),
            standing_pay_item_id,
        });
    }

    let mut new_lines = stored.clone();
    new_lines[existing_index].removed_reason = Some(reason.trim().to_owned());

    let wrote =
        write_member_pay_lines(&mut tx, payroll_run_id, employment_id, &stored, new_lines).await?;
    if wrote {
        write_action_log_entry(
            &mut tx,
            ActionLogEntry {
                employer_id: &run.employer_id,
                actor,
                action_type: ActionType::StandingPayLineRemoved,
                target_type: "employment",
                target_id: employment_id.as_str(),
                context: Some(serde_json::json!({
                    "payroll_run_id": payroll_run_id.as_str(),
                    "standing_pay_item_id": standing_pay_item_id.as_str(),
                    "reason": reason,
                })),
            },
        )
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

/// One member's own report from a [`refresh_standing_proposals`] call: what
/// was added and updated, and what was deliberately left alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberProposalRefresh {
    pub employment_id: EmploymentId,
    pub added: Vec<StandingItemProposal>,
    pub updated: Vec<StandingItemProposal>,
    pub kept_overridden: Vec<StandingPayItemId>,
    pub kept_removed: Vec<StandingPayItemId>,
    pub ended_still_proposed: Vec<StandingItemProposal>,
}

/// What [`refresh_standing_proposals`] returns: only the members that had
/// something to report. A member whose proposals already match the
/// standing records exactly — nothing added, changed, ended, overridden or
/// removed — is not listed at all, so an empty `members` is itself the
/// answer "nothing changed".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandingProposalRefresh {
    pub members: Vec<MemberProposalRefresh>,
}

/// Refreshes every active member's standing proposals on `payroll_run_id`
/// against what is in force right now (§0, §D-6): adds a line for a newly
/// in-force item, restates a plain line whose item's instruction changed
/// (unreachable today — see [`StandingItemsChangedSinceProposal::changed`]),
/// and leaves every overridden and every removed line exactly as it is.
/// **Never re-proposes anything and never runs on its own** — it is always
/// an explicit operator action, and `calculate_payroll_run` never calls
/// anything in this module.
///
/// Refused outright when the run is not editable
/// ([`PayrollAppError::PayrollRunAlreadyFinalized`]) or is not Ordinary
/// ([`PayrollAppError::PayrollRunIsNotOrdinary`]) — a Correction run never
/// proposes, so it has nothing to refresh.
///
/// Runs under the run's own lock, then locks the governing Employer row
/// `FOR SHARE` — the same order [`crate::finalize_payroll_run`] takes (run,
/// then employer) — so `end_standing_pay_item`'s own `FOR UPDATE` on that
/// row cannot commit half way through this refresh: either the ended item
/// is already gone when this reads `StandingPayItem`s in force, or the end
/// waits for this transaction to finish first. All in one transaction, so
/// two concurrent refreshes of the same run add each newly in-force item
/// exactly once, never twice.
///
/// Writes one `ActionLog` entry, naming the whole report, only when at
/// least one member's lines actually changed; if nothing was written, no
/// entry is written and the run's `status` is untouched (a `Calculated` run
/// stays `Calculated`).
pub async fn refresh_standing_proposals(
    db: &SaltDatabase,
    payroll_run_id: &PayrollRunId,
    actor: &str,
) -> Result<StandingProposalRefresh, PayrollAppError> {
    let mut tx = db.pool().begin().await?;
    let run = lock_editable_run(&mut tx, payroll_run_id).await?;
    if run.kind != RunKind::Ordinary {
        return Err(PayrollAppError::PayrollRunIsNotOrdinary(
            payroll_run_id.clone(),
        ));
    }

    // See this function's own doc: the same run-then-employer lock order
    // `finalize_payroll_run` takes, so an `end_standing_pay_item` cannot
    // interleave with this refresh.
    sqlx::query("SELECT 1 FROM employer WHERE id = $1 FOR SHARE")
        .bind(run.employer_id.as_str())
        .fetch_optional(&mut *tx)
        .await?;

    let member_ids = active_member_ids(&mut tx, payroll_run_id).await?;

    let mut members_report = Vec::new();
    let mut any_write = false;
    for member_id in member_ids {
        let employment_id = EmploymentId::new(member_id);
        let stored = read_member_pay_lines(&mut tx, payroll_run_id, &employment_id).await?;
        let in_force = crate::standing_pay_item::standing_pay_lines_in_force(
            &mut tx,
            &employment_id,
            run.period.end(),
        )
        .await?
        .into_iter()
        .map(
            |(standing_pay_item_id, pay_line_json, effective_from)| StandingItemProposal {
                standing_pay_item_id,
                instruction: StandingPayItemInstruction::from_pay_line_json(pay_line_json),
                effective_from,
            },
        )
        .collect::<Vec<_>>();

        let diff = diff_standing_proposal(&stored, &in_force);

        let mut new_lines = stored.clone();
        for added in &diff.added {
            let instruction = added.instruction.clone().into_pay_line_instruction();
            new_lines.push(RunPayLine {
                instruction: instruction.clone(),
                source: PayLineSource::Standing,
                standing_pay_item_id: Some(added.standing_pay_item_id.clone()),
                standing_effective_from: Some(added.effective_from),
                override_reason: None,
                removed_reason: None,
                standing_instruction: Some(instruction),
            });
        }
        for changed in &diff.changed {
            let instruction = changed.instruction.clone().into_pay_line_instruction();
            if let Some(line) = new_lines.iter_mut().find(|line| {
                line.standing_pay_item_id.as_ref() == Some(&changed.standing_pay_item_id)
            }) {
                line.instruction = instruction.clone();
                line.standing_instruction = Some(instruction);
            }
        }

        let kept_overridden: Vec<StandingPayItemId> = stored
            .iter()
            .filter(|line| {
                line.source == PayLineSource::Standing
                    && !line.is_removed()
                    && line.override_reason.is_some()
            })
            .map(|line| {
                line.standing_pay_item_id
                    .clone()
                    .expect("a Standing-sourced line always carries a StandingPayItemId")
            })
            .collect();
        let kept_removed: Vec<StandingPayItemId> = stored
            .iter()
            .filter(|line| line.source == PayLineSource::Standing && line.is_removed())
            .map(|line| {
                line.standing_pay_item_id
                    .clone()
                    .expect("a Standing-sourced line always carries a StandingPayItemId")
            })
            .collect();

        let wrote =
            write_member_pay_lines(&mut tx, payroll_run_id, &employment_id, &stored, new_lines)
                .await?;
        any_write |= wrote;

        let has_something_to_report = !diff.added.is_empty()
            || !diff.changed.is_empty()
            || !kept_overridden.is_empty()
            || !kept_removed.is_empty()
            || !diff.ended.is_empty();
        if has_something_to_report {
            members_report.push(MemberProposalRefresh {
                employment_id,
                added: diff.added,
                updated: diff.changed,
                kept_overridden,
                kept_removed,
                ended_still_proposed: diff.ended,
            });
        }
    }

    if any_write {
        let context = serde_json::json!({
            "members": members_report.iter().map(|member| {
                serde_json::json!({
                    "employment_id": member.employment_id.as_str(),
                    "added": member.added.iter()
                        .map(|item| item.standing_pay_item_id.as_str())
                        .collect::<Vec<_>>(),
                    "updated": member.updated.iter()
                        .map(|item| item.standing_pay_item_id.as_str())
                        .collect::<Vec<_>>(),
                    "kept_overridden": member.kept_overridden.iter()
                        .map(StandingPayItemId::as_str)
                        .collect::<Vec<_>>(),
                    "kept_removed": member.kept_removed.iter()
                        .map(StandingPayItemId::as_str)
                        .collect::<Vec<_>>(),
                    "ended_still_proposed": member.ended_still_proposed.iter()
                        .map(|item| item.standing_pay_item_id.as_str())
                        .collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
        });
        write_action_log_entry(
            &mut tx,
            ActionLogEntry {
                employer_id: &run.employer_id,
                actor,
                action_type: ActionType::StandingProposalsRefreshed,
                target_type: "payroll_run",
                target_id: payroll_run_id.as_str(),
                context: Some(context),
            },
        )
        .await?;
    }

    tx.commit().await?;
    Ok(StandingProposalRefresh {
        members: members_report,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use payroll::EarningLabel;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn allowance_instruction(cents: i64) -> StandingPayItemInstruction {
        StandingPayItemInstruction::TaxableAllowance {
            amount: Money::from_cents(cents).unwrap(),
            label: EarningLabel::new("standby").unwrap(),
        }
    }

    fn item(id: &str, cents: i64) -> StandingItemProposal {
        StandingItemProposal {
            standing_pay_item_id: StandingPayItemId::new(id),
            instruction: allowance_instruction(cents),
            effective_from: date(2026, 1, 1),
        }
    }

    fn standing_line(id: &str, cents: i64) -> RunPayLine {
        RunPayLine {
            instruction: allowance_instruction(cents).into_pay_line_instruction(),
            source: PayLineSource::Standing,
            standing_pay_item_id: Some(StandingPayItemId::new(id)),
            standing_effective_from: Some(date(2026, 1, 1)),
            override_reason: None,
            removed_reason: None,
            standing_instruction: Some(allowance_instruction(cents).into_pay_line_instruction()),
        }
    }

    #[test]
    fn algorithm_an_item_in_force_with_no_line_is_added() {
        let in_force = vec![item("a", 20_000)];
        let diff = diff_standing_proposal(&[], &in_force);
        assert_eq!(diff.added, in_force);
        assert!(diff.changed.is_empty());
        assert!(diff.ended.is_empty());
    }

    #[test]
    fn algorithm_a_line_whose_item_ended_is_ended() {
        let lines = vec![standing_line("a", 20_000)];
        let diff = diff_standing_proposal(&lines, &[]);
        assert_eq!(diff.ended, vec![item("a", 20_000)]);
        assert!(diff.added.is_empty());
        assert!(diff.changed.is_empty());
    }

    #[test]
    fn algorithm_a_plain_line_whose_instruction_differs_is_changed() {
        let lines = vec![standing_line("a", 20_000)];
        let in_force = vec![item("a", 25_000)];
        let diff = diff_standing_proposal(&lines, &in_force);
        assert_eq!(diff.changed, vec![item("a", 25_000)]);
        assert!(diff.added.is_empty());
        assert!(diff.ended.is_empty());
    }

    #[test]
    fn algorithm_an_overridden_line_is_never_changed_but_can_be_ended() {
        let mut overridden = standing_line("a", 20_000);
        overridden.instruction = allowance_instruction(30_000).into_pay_line_instruction();
        overridden.override_reason = Some("temporary raise".to_string());

        let still_in_force = diff_standing_proposal(&[overridden.clone()], &[item("a", 20_000)]);
        assert!(still_in_force.changed.is_empty());
        assert!(still_in_force.ended.is_empty());

        let no_longer_in_force = diff_standing_proposal(&[overridden], &[]);
        assert_eq!(no_longer_in_force.ended, vec![item("a", 20_000)]);
    }

    #[test]
    fn algorithm_a_removed_line_is_neither_added_changed_nor_ended() {
        let mut removed = standing_line("a", 20_000);
        removed.removed_reason = Some("not paid this month".to_string());

        let diff_with_item_in_force =
            diff_standing_proposal(&[removed.clone()], &[item("a", 20_000)]);
        assert!(diff_with_item_in_force.is_empty());

        let diff_item_ended = diff_standing_proposal(&[removed], &[]);
        assert!(diff_item_ended.is_empty());
    }

    #[test]
    fn algorithm_nothing_changed_is_empty() {
        let lines = vec![standing_line("a", 20_000)];
        let in_force = vec![item("a", 20_000)];
        assert!(diff_standing_proposal(&lines, &in_force).is_empty());
    }
}
