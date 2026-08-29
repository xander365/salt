//! `DeclareUnsupportedDeductionStatus` (§4.5c, §12).

use chrono::NaiveDate;
use payroll::{
    EmployerId, EmploymentId, UnsupportedDeductionStatus, validate_effective_from_is_a_period_start,
};
use sqlx::PgPool;

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::employer::pay_schedule_from_columns;
use crate::error::PayrollAppError;

/// Records the `UnsupportedDeductionDeclaration` in force from
/// `effective_from`, or replaces whichever one already governs that exact
/// date. Effective-dated, unlike `PriorEmployment` (§4.5b): this genuinely
/// changes mid-year — an employee joining a provident fund in August is
/// exactly the case §4.5c names.
///
/// `effective_from` must be one of the Employer's own `PayPeriod` start
/// dates (INV-014's shape, applied here so exactly one row ever governs a
/// period), checked in Rust before the row is written rather than left to
/// database arithmetic.
///
/// `status` must be `ConfirmedNone` or `Present`: the row itself is
/// two-valued, and `Unknown` is what no row in force at a period end
/// already means (§4.5c), so it is refused here rather than written. An
/// empty `kinds` collection is not representable in
/// `UnsupportedDeductionKinds`, so `Present` can never collapse into
/// `ConfirmedNone` by accident.
///
/// This writes no pre-finalization gate: `calculate` already refuses
/// `Unknown`, so a period with no declaration in force can never reach
/// `Calculated` (§4.5c).
pub async fn declare_unsupported_deduction_status(
    pool: &PgPool,
    employment_id: &EmploymentId,
    effective_from: NaiveDate,
    status: UnsupportedDeductionStatus,
    declared_by: &str,
) -> Result<(), PayrollAppError> {
    let (db_status, kinds) = match status {
        UnsupportedDeductionStatus::Unknown => {
            return Err(PayrollAppError::UnsupportedDeductionDeclarationCannotBeUnknown);
        }
        UnsupportedDeductionStatus::ConfirmedNone => ("confirmed_none", None),
        UnsupportedDeductionStatus::Present(kinds) => (
            "present",
            Some(
                serde_json::to_value(&kinds)
                    .expect("UnsupportedDeductionKinds serializes to a JSON array"),
            ),
        ),
    };

    let mut tx = pool.begin().await?;

    // `FOR SHARE OF employment` holds the row against a concurrent
    // `void_employment`, so a void committing between this read and the
    // insert cannot leave a declaration recorded against a now-voided
    // Employment.
    let employment: Option<(String, bool, String, Option<i16>)> = sqlx::query_as(
        "SELECT employment.employer_id,
                employment.is_void,
                employer.period_end_day_kind,
                employer.period_end_day_value
         FROM employment
         JOIN employer ON employer.id = employment.employer_id
         WHERE employment.id = $1
         FOR SHARE OF employment",
    )
    .bind(employment_id.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    let (employer_id, is_void, kind, value) =
        employment.ok_or_else(|| PayrollAppError::EmploymentNotFound(employment_id.clone()))?;
    if is_void {
        return Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()));
    }
    let employer_id = EmployerId::new(employer_id);
    let schedule = pay_schedule_from_columns(&kind, value);

    validate_effective_from_is_a_period_start(schedule, effective_from)?;

    sqlx::query(
        "INSERT INTO unsupported_deduction_declaration
            (employment_id, effective_from, status, kinds, declared_by)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (employment_id, effective_from) DO UPDATE
         SET status = EXCLUDED.status,
             kinds = EXCLUDED.kinds,
             declared_at = now(),
             declared_by = EXCLUDED.declared_by",
    )
    .bind(employment_id.as_str())
    .bind(effective_from)
    .bind(db_status)
    .bind(kinds)
    .bind(declared_by)
    .execute(&mut *tx)
    .await?;

    // Unlike `PriorEmployment`, the ActionType catalogue names only one act
    // for this declaration (`UnsupportedDeductionStatusCorrected`): every
    // write against a period — first declaration or later change — is a
    // correction to what governs that period, not a distinct kind of act.
    write_action_log_entry(
        &mut tx,
        ActionLogEntry {
            employer_id: &employer_id,
            actor: declared_by,
            action_type: ActionType::UnsupportedDeductionStatusCorrected,
            target_type: "employment",
            target_id: employment_id.as_str(),
            context: None,
        },
    )
    .await?;

    tx.commit().await?;
    Ok(())
}
