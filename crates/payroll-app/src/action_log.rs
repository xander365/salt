//! The `ActionLog` writer (`docs/domain/payroll-run-persistence.md` §10),
//! introduced by this ticket for every later use case to reuse.
//! `action_type` is a typed Rust enum, never free text, so an auditor never
//! has to parse a string — and the enum names exactly the thirteen acts the
//! design calls out, so the database's `action_type` CHECK and this type
//! can never quietly drift apart.

use chrono::{DateTime, Utc};
use payroll::EmployerId;

use crate::database::SaltDatabase;
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
    EmployerParticularsCorrected,
    PersonParticularsCorrected,
    PersonFullNameCorrected,
}

impl ActionType {
    /// Every variant, so a test can walk the whole enum and compare it with
    /// the database's own `action_type` CHECK.
    pub const ALL: [ActionType; 16] = [
        Self::PayrollRunCreated,
        Self::EmploymentRemovedFromRun,
        Self::EmploymentAddedToCorrectionRun,
        Self::PayrollFinalized,
        Self::FinalizedPayrollReversed,
        Self::OpeningBalanceCreated,
        Self::OpeningBalanceChanged,
        Self::PriorEmploymentDeclared,
        Self::PriorEmploymentChanged,
        Self::CompensationTermsCorrected,
        Self::UnsupportedDeductionStatusCorrected,
        Self::PayScheduleChanged,
        Self::EmploymentVoided,
        Self::EmployerParticularsCorrected,
        Self::PersonParticularsCorrected,
        Self::PersonFullNameCorrected,
    ];

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
            Self::EmployerParticularsCorrected => "employer_particulars_corrected",
            Self::PersonParticularsCorrected => "person_particulars_corrected",
            Self::PersonFullNameCorrected => "person_full_name_corrected",
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

/// One `ActionLogEntry`, read back for display (issue #72: the Employment
/// screen's own audit trail for a Person's name and particulars
/// corrections). `action_type` stays the raw database string here rather
/// than the typed enum: a read model shows what happened, and has no
/// occasion to construct an `ActionType` to write with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionLogEntryRecord {
    pub occurred_at: DateTime<Utc>,
    pub actor: String,
    pub action_type: String,
    pub context: Option<serde_json::Value>,
}

/// Every `ActionLogEntry` recorded against `(target_type, target_id)`,
/// newest first. Scoped to `employer_id` too, though `target_id` alone
/// already picks out at most one row's worth of history in every caller
/// today: the extra predicate is the same defence-in-depth ADR-0017 asks of
/// every other read here, at no real cost.
pub async fn list_action_log_entries_for_target(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    target_type: &str,
    target_id: &str,
) -> Result<Vec<ActionLogEntryRecord>, PayrollAppError> {
    type Row = (DateTime<Utc>, String, String, Option<serde_json::Value>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT occurred_at, actor, action_type, context
         FROM action_log_entry
         WHERE employer_id = $1 AND target_type = $2 AND target_id = $3
         ORDER BY occurred_at DESC, id DESC",
    )
    .bind(employer_id.as_str())
    .bind(target_type)
    .bind(target_id)
    .fetch_all(db.pool())
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(occurred_at, actor, action_type, context)| ActionLogEntryRecord {
                occurred_at,
                actor,
                action_type,
                context,
            },
        )
        .collect())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

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
            (
                ActionType::EmployerParticularsCorrected,
                "employer_particulars_corrected",
            ),
            (
                ActionType::PersonParticularsCorrected,
                "person_particulars_corrected",
            ),
            (
                ActionType::PersonFullNameCorrected,
                "person_full_name_corrected",
            ),
        ];

        for (action_type, expected_str) in expected {
            assert_eq!(action_type.as_db_str(), expected_str);
        }
        assert_eq!(
            expected.len(),
            ActionType::ALL.len(),
            "ActionType::ALL must list every variant"
        );
    }

    #[test]
    fn no_two_variants_share_a_string() {
        let strings: BTreeSet<&str> = ActionType::ALL.iter().map(|a| a.as_db_str()).collect();

        assert_eq!(
            strings.len(),
            ActionType::ALL.len(),
            "two ActionTypes writing one string would be two acts an auditor cannot tell apart"
        );
    }

    /// The enum and the database's CHECK are two statements of one list, so
    /// the only useful test compares them against each other rather than
    /// each against a copy of itself. Drift in either direction fails here:
    /// a variant PostgreSQL would refuse, and an accepted string no Rust
    /// caller can ever write.
    #[sqlx::test]
    async fn the_enum_and_the_check_constraint_name_the_same_acts(pool: sqlx::PgPool) {
        let definition: String = sqlx::query_scalar(
            "SELECT pg_get_constraintdef(oid) FROM pg_constraint
             WHERE conrelid = 'action_log_entry'::regclass
               AND contype = 'c'
               AND conname = 'action_log_entry_action_type_check'",
        )
        .fetch_one(&pool)
        .await
        .expect("action_log_entry.action_type carries a CHECK constraint");

        // Every literal in the definition is one accepted action type, and
        // `pg_get_constraintdef` renders each single-quoted, so the odd
        // fragments of a split on the quote character are exactly that set.
        let accepted: BTreeSet<&str> = definition.split('\'').skip(1).step_by(2).collect();
        let produced: BTreeSet<&str> = ActionType::ALL.iter().map(|a| a.as_db_str()).collect();

        assert_eq!(accepted, produced);
    }
}
