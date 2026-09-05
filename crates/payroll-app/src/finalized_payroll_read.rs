//! `GetFinalizedPayrollDetail` and `GetFinalizedPayrollTraces` (§0.29,
//! §0.30; issue #57, parent #49 Spec 2 of 3): an Operator opens a payroll
//! that has already been finalized and reads it back — the nine figures
//! (§0.29, [`crate::PayrollFigures`]), the period, the pay date and the
//! `SaltVersion` that produced them — without ever seeing the raw
//! `payroll_calculation_json` snapshot [`crate::finalize`] freezes.
//!
//! Both read models take `employer_id` explicitly and filter on it in SQL
//! (§0.30, ADR-0017): a `finalized_payroll_id` belonging to another Employer
//! is refused exactly like one that does not exist at all, both as
//! [`PayrollAppError::FinalizedPayrollNotFound`] — the same
//! "unknown and cross-Employer are indistinguishable" reasoning
//! [`crate::get_payroll_run_detail`] already applies to a `PayrollRunId`.
//!
//! Traces are deliberately a separate read model, and a separate route
//! (§0.29's own words: "PAYE and SSC traces are a separate endpoint") —
//! never inlined into the detail above "for convenience". The everyday
//! finalized-payroll view stays the nine figures; the workings are an
//! explicit second call.

use chrono::NaiveDate;
use payroll::{EmployerId, EmploymentId, PayPeriod, PayeTrace, PayrollCalculation, SscTrace};

use crate::database::SaltDatabase;
use crate::error::PayrollAppError;
use crate::finalize::FinalizedPayrollId;
use crate::payroll_run::PayrollFigures;

/// Wraps a caller-supplied id as a [`FinalizedPayrollId`], refusing a string
/// that is not even a well-formed UUID as
/// [`PayrollAppError::FinalizedPayrollNotFound`] — the same reasoning
/// [`crate::payroll_run::verify_payroll_run_belongs_to_employer`]'s own
/// `parse_payroll_run_id` applies: the `id` column's `::uuid` cast would
/// otherwise fail as a database error, turning a client's malformed path
/// segment into a 500 rather than the 404 it deserves.
fn parse_finalized_payroll_id(
    finalized_payroll_id: &str,
) -> Result<FinalizedPayrollId, PayrollAppError> {
    if uuid::Uuid::parse_str(finalized_payroll_id).is_err() {
        return Err(PayrollAppError::FinalizedPayrollNotFound(
            FinalizedPayrollId::new(finalized_payroll_id),
        ));
    }
    Ok(FinalizedPayrollId::new(finalized_payroll_id))
}

/// One `FinalizedPayroll` in full, for `GET
/// /api/employers/{e}/finalized-payroll/{f}` (issue #57): the nine figures,
/// the period, the pay date and the `SaltVersion` that produced them. Never
/// the raw `payroll_input_json`, `payroll_rules_json` or
/// `payroll_calculation_json` snapshot — §0.29's whole point is that a
/// snapshot column serialized straight out becomes a permanent public
/// contract by accident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizedPayrollDetail {
    pub id: FinalizedPayrollId,
    pub employment_id: EmploymentId,
    pub period: PayPeriod,
    pub pay_date: NaiveDate,
    pub figures: PayrollFigures,
    pub salt_version: String,
}

/// Reads one `FinalizedPayroll` back for display, scoped to `employer_id` in
/// SQL (ADR-0017). `payroll_calculation_json` is read and deserialized here
/// so [`PayrollFigures::from_calculation`] can extract the nine figures —
/// the deserialized value itself never leaves this function.
///
/// `pay_date` has no column of its own on `finalized_payroll` (§9): it is
/// the owning `PayrollRun`'s, joined in, exactly as frozen at finalization —
/// a `PayrollRun` is never edited after it finalizes (ADR-0004, ADR-0011),
/// so this join can never disagree with what was true at finalize time.
pub async fn get_finalized_payroll_detail(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    finalized_payroll_id: &str,
) -> Result<FinalizedPayrollDetail, PayrollAppError> {
    let finalized_payroll_id = parse_finalized_payroll_id(finalized_payroll_id)?;

    type Row = (
        String,
        NaiveDate,
        NaiveDate,
        NaiveDate,
        serde_json::Value,
        String,
    );

    let row: Option<Row> = sqlx::query_as(
        "SELECT finalized_payroll.employment_id, finalized_payroll.period_start,
                finalized_payroll.period_end, payroll_run.pay_date,
                finalized_payroll.payroll_calculation_json, finalized_payroll.salt_version
         FROM finalized_payroll
         JOIN payroll_run ON payroll_run.id = finalized_payroll.payroll_run_id
         WHERE finalized_payroll.id = $1::uuid AND finalized_payroll.employer_id = $2",
    )
    .bind(finalized_payroll_id.as_str())
    .bind(employer_id.as_str())
    .fetch_optional(db.pool())
    .await?;

    let (employment_id, period_start, period_end, pay_date, calculation_json, salt_version) =
        row.ok_or_else(|| PayrollAppError::FinalizedPayrollNotFound(finalized_payroll_id.clone()))?;

    let calculation: PayrollCalculation = serde_json::from_value(calculation_json).expect(
        "finalized_payroll.payroll_calculation_json always serializes a PayrollCalculation at \
         the snapshot_schema_version this code reads (§9.1)",
    );

    Ok(FinalizedPayrollDetail {
        id: finalized_payroll_id,
        employment_id: EmploymentId::new(employment_id),
        period: PayPeriod::new(period_start, period_end)
            .expect("finalized_payroll CHECK: period_end is never before period_start"),
        pay_date,
        figures: PayrollFigures::from_calculation(&calculation),
        salt_version,
    })
}

/// The PAYE and SSC workings behind one `FinalizedPayroll`'s figures, for
/// `GET /api/employers/{e}/finalized-payroll/{f}/traces` (issue #57): a
/// separate read model and a separate endpoint from
/// [`FinalizedPayrollDetail`] above (§0.29), so the everyday finalized view
/// stays readable and a workings screen is an explicit second call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizedPayrollTraces {
    pub paye: PayeTrace,
    pub employee_social_security: SscTrace,
    pub employer_social_security: SscTrace,
}

/// Reads one `FinalizedPayroll`'s traces back, scoped to `employer_id` in
/// SQL (ADR-0017) — the same scoping [`get_finalized_payroll_detail`]
/// applies, kept as its own query rather than sharing one with it: the two
/// routes are independently reachable, and neither's shape depends on the
/// other having been called first.
pub async fn get_finalized_payroll_traces(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    finalized_payroll_id: &str,
) -> Result<FinalizedPayrollTraces, PayrollAppError> {
    let finalized_payroll_id = parse_finalized_payroll_id(finalized_payroll_id)?;

    let calculation_json: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payroll_calculation_json
         FROM finalized_payroll
         WHERE id = $1::uuid AND employer_id = $2",
    )
    .bind(finalized_payroll_id.as_str())
    .bind(employer_id.as_str())
    .fetch_optional(db.pool())
    .await?;

    let calculation_json = calculation_json
        .ok_or_else(|| PayrollAppError::FinalizedPayrollNotFound(finalized_payroll_id.clone()))?;
    let calculation: PayrollCalculation = serde_json::from_value(calculation_json).expect(
        "finalized_payroll.payroll_calculation_json always serializes a PayrollCalculation at \
         the snapshot_schema_version this code reads (§9.1)",
    );

    Ok(FinalizedPayrollTraces {
        paye: calculation.paye.trace,
        employee_social_security: calculation.employee_social_security.trace,
        employer_social_security: calculation.employer_social_security.trace,
    })
}
