//! The `ActionLog` writer (`docs/domain/payroll-run-persistence.md` §10),
//! introduced by this ticket for every later use case to reuse.
//! `action_type` is a typed Rust enum, never free text, so an auditor never
//! has to parse a string — and the enum names exactly the thirteen acts the
//! design calls out, so the database's `action_type` CHECK and this type
//! can never quietly drift apart.

use payroll::EmployerId;

use crate::error::PayrollAppError;

/// One of the acts §10 names as worth an audit-trail entry. Only
/// `EmploymentVoided` is written by this ticket; the rest exist so this
/// enum matches the database CHECK exactly and every later use case has a
/// variant already waiting for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionType {
    PayrollRunCreated,
    EmploymentRemovedFromRun,
    EmploymentAddedToCorrectionRun,
    PayrollFinalized,
    FinalizedPayrollReversed,
    OpeningBalanceCreated,
    OpeningBalanceChanged,
    PriorEmploymentDeclared,
    PriorEmploymentChanged,
    CompensationTermsCorrected,
    UnsupportedDeductionStatusCorrected,
    PayScheduleChanged,
    EmploymentVoided,
}

impl ActionType {
    /// The exact string `action_log_entry.action_type`'s CHECK accepts.
    fn as_db_str(self) -> &'static str {
        match self {
            Self::PayrollRunCreated => "payroll_run_created",
            Self::EmploymentRemovedFromRun => "employment_removed_from_run",
            Self::EmploymentAddedToCorrectionRun => "employment_added_to_correction_run",
            Self::PayrollFinalized => "payroll_finalized",
            Self::FinalizedPayrollReversed => "finalized_payroll_reversed",
            Self::OpeningBalanceCreated => "opening_balance_created",
            Self::OpeningBalanceChanged => "opening_balance_changed",
            Self::PriorEmploymentDeclared => "prior_employment_declared",
            Self::PriorEmploymentChanged => "prior_employment_changed",
            Self::CompensationTermsCorrected => "compensation_terms_corrected",
            Self::UnsupportedDeductionStatusCorrected => "unsupported_deduction_status_corrected",
            Self::PayScheduleChanged => "pay_schedule_changed",
            Self::EmploymentVoided => "employment_voided",
        }
    }
}

/// One `ActionLogEntry`, ready to write. `target_type` names the kind of
/// thing `target_id` identifies (e.g. `"employment"`).
pub struct ActionLogEntry<'a> {
    pub employer_id: &'a EmployerId,
    pub actor: &'a str,
    pub action_type: ActionType,
    pub target_type: &'a str,
    pub target_id: &'a str,
    pub context: Option<serde_json::Value>,
}

/// Appends one row to `action_log_entry`. Takes the open connection or
/// transaction a caller is already writing the logged fact through — §10
/// requires the fact and its audit entry to commit together, so this never
/// opens a transaction of its own.
pub(crate) async fn write_action_log_entry(
    conn: &mut sqlx::PgConnection,
    entry: ActionLogEntry<'_>,
) -> Result<(), PayrollAppError> {
    sqlx::query(
        "INSERT INTO action_log_entry
            (employer_id, actor, action_type, target_type, target_id, context)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(entry.employer_id.as_str())
    .bind(entry.actor)
    .bind(entry.action_type.as_db_str())
    .bind(entry.target_type)
    .bind(entry.target_id)
    .bind(entry.context)
    .execute(conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The thirteen strings the migration's CHECK constraint names
    /// (`0014_action_log_entry.sql`). A typo here, or a variant this test
    /// forgets, is exactly the drift the typed enum exists to prevent.
    #[test]
    fn every_variant_maps_to_a_string_the_check_constraint_accepts() {
        let expected = [
            (ActionType::PayrollRunCreated, "payroll_run_created"),
            (
                ActionType::EmploymentRemovedFromRun,
                "employment_removed_from_run",
            ),
            (
                ActionType::EmploymentAddedToCorrectionRun,
                "employment_added_to_correction_run",
            ),
            (ActionType::PayrollFinalized, "payroll_finalized"),
            (
                ActionType::FinalizedPayrollReversed,
                "finalized_payroll_reversed",
            ),
            (ActionType::OpeningBalanceCreated, "opening_balance_created"),
            (ActionType::OpeningBalanceChanged, "opening_balance_changed"),
            (
                ActionType::PriorEmploymentDeclared,
                "prior_employment_declared",
            ),
            (
                ActionType::PriorEmploymentChanged,
                "prior_employment_changed",
            ),
            (
                ActionType::CompensationTermsCorrected,
                "compensation_terms_corrected",
            ),
            (
                ActionType::UnsupportedDeductionStatusCorrected,
                "unsupported_deduction_status_corrected",
            ),
            (ActionType::PayScheduleChanged, "pay_schedule_changed"),
            (ActionType::EmploymentVoided, "employment_voided"),
        ];

        for (action_type, expected_str) in expected {
            assert_eq!(action_type.as_db_str(), expected_str);
        }
    }
}
