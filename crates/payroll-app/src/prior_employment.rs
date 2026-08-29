//! `DeclarePriorEmployment` (§4.5b, §12).

use payroll::{EmployerId, EmploymentId, PriorEmployment, TaxYear};
use sqlx::PgPool;

use crate::action_log::{ActionLogEntry, ActionType, write_action_log_entry};
use crate::error::PayrollAppError;

/// Records the `PriorEmploymentDeclaration` for one (Employment, TaxYear),
/// or replaces whichever one is already there. Concurrent employment is out
/// of scope (§4.5b), so this is Employment+TaxYear state — never
/// effective-dated and never run-scoped, because the fact it states cannot
/// change during the year.
///
/// `prior_employment` must be `None` or `Some`: the row itself is
/// two-valued, and `Unknown` is what the absence of a row already means
/// (§4.5b), so it is refused here rather than written.
///
/// This writes no pre-finalization gate and enforces no freeze: `calculate`
/// already refuses `Unknown`, so a run missing this declaration can never
/// reach `Calculated` (§4.5b). Freezing an existing declaration once it has
/// been read into a finalization is a later ticket.
pub async fn declare_prior_employment(
    pool: &PgPool,
    employment_id: &EmploymentId,
    tax_year: TaxYear,
    prior_employment: PriorEmployment,
    declared_by: &str,
) -> Result<(), PayrollAppError> {
    let (status, taxable_remuneration, paye) = match prior_employment {
        PriorEmployment::Unknown => {
            return Err(PayrollAppError::PriorEmploymentDeclarationCannotBeUnknown);
        }
        PriorEmployment::None => ("confirmed_none", None, None),
        PriorEmployment::Some(figures) => (
            "present",
            Some(figures.taxable_remuneration().cents()),
            Some(figures.paye().cents()),
        ),
    };

    let mut tx = pool.begin().await?;

    // `FOR SHARE` holds the row against a concurrent `void_employment`, so a
    // void committing between this read and the insert cannot leave a
    // declaration recorded against a now-voided Employment.
    let employment: Option<(String, bool)> =
        sqlx::query_as("SELECT employer_id, is_void FROM employment WHERE id = $1 FOR SHARE")
            .bind(employment_id.as_str())
            .fetch_optional(&mut *tx)
            .await?;
    let (employer_id, is_void) =
        employment.ok_or_else(|| PayrollAppError::EmploymentNotFound(employment_id.clone()))?;
    if is_void {
        return Err(PayrollAppError::EmploymentIsVoid(employment_id.clone()));
    }
    let employer_id = EmployerId::new(employer_id);

    // `xmax = 0` is true only for the row version this statement itself
    // inserted; an update leaves the prior version's `xmax` set. That is
    // what distinguishes a first declaration from a changed one, in the
    // same round trip as the write.
    let inserted: bool = sqlx::query_scalar(
        "INSERT INTO prior_employment_declaration
            (employment_id, tax_year, status, taxable_remuneration, paye, declared_by)
         VALUES ($1, $2, $3, $4, $5, $6)
         ON CONFLICT (employment_id, tax_year) DO UPDATE
         SET status = EXCLUDED.status,
             taxable_remuneration = EXCLUDED.taxable_remuneration,
             paye = EXCLUDED.paye,
             declared_at = now(),
             declared_by = EXCLUDED.declared_by
         RETURNING (xmax = 0)",
    )
    .bind(employment_id.as_str())
    .bind(tax_year.starting_year())
    .bind(status)
    .bind(taxable_remuneration)
    .bind(paye)
    .bind(declared_by)
    .fetch_one(&mut *tx)
    .await?;

    write_action_log_entry(
        &mut tx,
        ActionLogEntry {
            employer_id: &employer_id,
            actor: declared_by,
            action_type: if inserted {
                ActionType::PriorEmploymentDeclared
            } else {
                ActionType::PriorEmploymentChanged
            },
            target_type: "employment",
            target_id: employment_id.as_str(),
            context: None,
        },
    )
    .await?;

    tx.commit().await?;
    Ok(())
}
