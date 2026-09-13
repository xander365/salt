//! The frozen pay-line provenance snapshot (issue #80, parent #70 §D-6):
//! each line's source and its override/removal reason, exactly as
//! `payroll_run_pay_line` held them at finalization. Frozen because an old
//! payroll's workings must never be rebuilt from today's standing records
//! (ADR-0004) — labels live inside the domain type and are therefore
//! already inside `payroll_input_json`; provenance and override reasons are
//! operational facts of their own, so they get their own column,
//! `finalized_payroll.pay_line_provenance_json` (migration 0041).

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::payroll_run::{PayLineInstruction, PayLineSource, RunPayLine};

/// One line's frozen provenance, in the run's stored (canonical) order —
/// removed lines included, so an old payroll's workings can still say "this
/// was removed for this run, reason X" without reading any current standing
/// record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FrozenPayLine {
    pub pay_line: PayLineInstruction,
    pub source: PayLineSource,
    pub standing_pay_item_id: Option<String>,
    pub standing_effective_from: Option<NaiveDate>,
    pub standing_pay_line: Option<PayLineInstruction>,
    pub override_reason: Option<String>,
    pub removed_reason: Option<String>,
}

/// The whole snapshot: `{"lines": [...]}` — a wrapper object, not a bare
/// array, so a future sibling field has somewhere to go without changing
/// the shape every existing row already carries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayLineProvenanceSnapshot {
    pub lines: Vec<FrozenPayLine>,
}

/// Builds the snapshot from one member's complete, canonically ordered pay
/// lines — the same lines [`crate::payroll_run::read_member_pay_lines`]
/// returns, frozen verbatim rather than re-derived from today's standing
/// records.
pub(crate) fn build_pay_line_provenance(lines: &[RunPayLine]) -> PayLineProvenanceSnapshot {
    PayLineProvenanceSnapshot {
        lines: lines
            .iter()
            .map(|line| FrozenPayLine {
                pay_line: line.instruction.clone(),
                source: line.source,
                standing_pay_item_id: line
                    .standing_pay_item_id
                    .as_ref()
                    .map(|id| id.as_str().to_string()),
                standing_effective_from: line.standing_effective_from,
                standing_pay_line: line.standing_instruction.clone(),
                override_reason: line.override_reason.clone(),
                removed_reason: line.removed_reason.clone(),
            })
            .collect(),
    }
}
