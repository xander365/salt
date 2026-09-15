//! `GetPayslipData` (issue #82, parent #70): everything a Payslip renderer
//! needs for one `FinalizedPayroll`, and nothing it does not. The renderer
//! itself lives in `salt-server` (Deep Instructions, issue #82) and reads no
//! database at all — this is the one function that reads the row and hands
//! back a plain, already-frozen struct.
//!
//! Two rules this module exists to enforce:
//!
//! - **The frozen particulars are mandatory here, unlike on
//!   [`crate::FinalizedPayrollDetail`].** That read model shows an Operator
//!   whatever a payroll has, including "nothing froze here" for a
//!   pre-issue-#73 row. A Payslip cannot: printing a document with an
//!   employer address, an employee identity number or a template version
//!   silently blank would be a statutory document Salt cannot stand behind.
//!   [`get_payslip_data`] refuses instead, naming exactly which of the three
//!   is missing (Deep Instructions: the refusal keys on absence, never on
//!   `snapshot_schema_version`).
//! - **No particular is ever read from a current master record.** Every
//!   field on [`PayslipData`] comes from the frozen columns
//!   `finalize_payroll_run` wrote, or from the frozen
//!   `payroll_calculation_json` itself — never from a live join to
//!   `employer_particulars`, `person_particulars` or `employment` beyond the
//!   id and period `finalized_payroll` already owns. A later correction to
//!   an address or a name must never reach a payslip already issued.

use chrono::NaiveDate;
use payroll::{Deduction, Earning, EmployerId, EmploymentId, PayPeriod};

use crate::database::SaltDatabase;
use crate::error::PayrollAppError;
use crate::finalize::FinalizedPayrollId;
use crate::finalized_payroll_read::{
    FinalizedEmployerParticulars, FinalizedPersonParticulars, calculation_from_snapshot,
    parse_finalized_payroll_id, particulars_from_snapshot,
};
use crate::payroll_run::PayrollFigures;

/// What a Payslip must say about reversal and replacement (issue #82,
/// CONTEXT.md's own `Reversal`/`Replacement` entries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayslipReversal {
    /// The reason recorded on the `Reversal` (§6.1: always non-blank).
    pub reason: String,
    /// The `FinalizedPayroll` that took this one's place, if any — a
    /// reversal is complete on its own (CONTEXT.md's `Replacement` entry),
    /// so this is `None` until, and unless, a Correction names this record
    /// as its target.
    pub replacement_id: Option<FinalizedPayrollId>,
}

/// Everything a Payslip renderer needs for one `FinalizedPayroll`, entirely
/// frozen at finalize time. See the module doc for what makes every field
/// here mandatory where [`crate::FinalizedPayrollDetail`]'s equivalents are
/// optional.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayslipData {
    pub id: FinalizedPayrollId,
    pub employment_id: EmploymentId,
    pub period: PayPeriod,
    pub pay_date: NaiveDate,
    pub salt_version: String,
    pub payslip_template_version: String,
    pub employer_particulars: FinalizedEmployerParticulars,
    pub person_particulars: FinalizedPersonParticulars,
    /// Every earning line the frozen calculation carries, in the order it
    /// carries them — `BasicPay` first, exactly as `calculate` produced it.
    pub earning_lines: Vec<Earning>,
    /// Every deduction the frozen calculation carries, statutory first and
    /// voluntary last (`payroll::PayrollCalculation::deductions`'s own
    /// promise) — the order a payslip prints.
    pub deductions: Vec<Deduction>,
    /// The ten summary figures, read the same way
    /// [`crate::FinalizedPayrollDetail::figures`] is.
    pub figures: PayrollFigures,
    /// Present when this row is itself a Replacement (CONTEXT.md): the
    /// `FinalizedPayroll` it replaces.
    pub replaces: Option<FinalizedPayrollId>,
    /// Present when this row has been reversed.
    pub reversal: Option<PayslipReversal>,
}

/// Reads everything [`PayslipData`] needs in one query, scoped to
/// `employer_id` in SQL (ADR-0017) — the same "unknown and cross-Employer
/// are indistinguishable" reasoning [`crate::get_finalized_payroll_detail`]
/// already applies.
///
/// Refused as [`PayrollAppError::PayslipParticularsNotFrozen`] when any of
/// the three frozen columns this type demands is absent — a row finalized
/// before issue #73. Refused as
/// [`PayrollAppError::FinalizedPayrollSnapshotUnreadable`] when a present
/// column does not decode, exactly as every other frozen-snapshot reader in
/// this crate already answers that.
pub async fn get_payslip_data(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    finalized_payroll_id: &str,
) -> Result<PayslipData, PayrollAppError> {
    let finalized_payroll_id = parse_finalized_payroll_id(finalized_payroll_id)?;

    type Row = (
        String,                    // employment_id
        NaiveDate,                 // period_start
        NaiveDate,                 // period_end
        NaiveDate,                 // pay_date
        i32,                       // snapshot_schema_version
        serde_json::Value,         // payroll_calculation_json
        String,                    // salt_version
        Option<serde_json::Value>, // employer_particulars_json
        Option<serde_json::Value>, // person_particulars_json
        Option<String>,            // payslip_template_version
        Option<String>,            // replaces_finalized_payroll_id
        Option<String>,            // reversal.reason
        Option<String>,            // the id of whatever replaces this row
    );

    let row: Option<Row> = sqlx::query_as(
        "SELECT finalized_payroll.employment_id, finalized_payroll.period_start,
                finalized_payroll.period_end, payroll_run.pay_date,
                finalized_payroll.snapshot_schema_version,
                finalized_payroll.payroll_calculation_json, finalized_payroll.salt_version,
                finalized_payroll.employer_particulars_json,
                finalized_payroll.person_particulars_json,
                finalized_payroll.payslip_template_version,
                finalized_payroll.replaces_finalized_payroll_id::text,
                reversal.reason,
                replacement.id::text
         FROM finalized_payroll
         JOIN payroll_run ON payroll_run.id = finalized_payroll.payroll_run_id
         LEFT JOIN reversal ON reversal.finalized_payroll_id = finalized_payroll.id
         LEFT JOIN finalized_payroll AS replacement
                ON replacement.replaces_finalized_payroll_id = finalized_payroll.id
         WHERE finalized_payroll.id = $1::uuid AND finalized_payroll.employer_id = $2",
    )
    .bind(finalized_payroll_id.as_str())
    .bind(employer_id.as_str())
    .fetch_optional(db.pool())
    .await?;

    let (
        employment_id,
        period_start,
        period_end,
        pay_date,
        schema_version,
        calculation_json,
        salt_version,
        employer_particulars_json,
        person_particulars_json,
        payslip_template_version,
        replaces_finalized_payroll_id,
        reversal_reason,
        replacement_id,
    ) =
        row.ok_or_else(|| PayrollAppError::FinalizedPayrollNotFound(finalized_payroll_id.clone()))?;

    let employer_particulars: Option<FinalizedEmployerParticulars> = particulars_from_snapshot(
        &finalized_payroll_id,
        schema_version,
        employer_particulars_json,
    )?;
    let person_particulars: Option<FinalizedPersonParticulars> = particulars_from_snapshot(
        &finalized_payroll_id,
        schema_version,
        person_particulars_json,
    )?;

    let mut missing = Vec::new();
    if employer_particulars.is_none() {
        missing.push("EmployerParticulars");
    }
    if person_particulars.is_none() {
        missing.push("PersonParticulars");
    }
    if payslip_template_version.is_none() {
        missing.push("PayslipTemplateVersion");
    }
    if !missing.is_empty() {
        return Err(PayrollAppError::PayslipParticularsNotFrozen {
            finalized_payroll_id,
            missing,
        });
    }
    // `missing` being empty is exactly the condition that makes every
    // `.expect()` below unreachable in practice: each pushed to `missing`
    // when, and only when, its own `Option` is `None`.
    let employer_particulars =
        employer_particulars.expect("checked via `missing` above: EmployerParticulars is Some");
    let person_particulars =
        person_particulars.expect("checked via `missing` above: PersonParticulars is Some");
    let payslip_template_version = payslip_template_version
        .expect("checked via `missing` above: PayslipTemplateVersion is Some");

    let calculation =
        calculation_from_snapshot(&finalized_payroll_id, schema_version, calculation_json)?;

    let reversal = reversal_reason.map(|reason| PayslipReversal {
        reason,
        replacement_id: replacement_id.map(FinalizedPayrollId::new),
    });

    let figures = PayrollFigures::from_calculation(&calculation);

    Ok(PayslipData {
        id: finalized_payroll_id,
        employment_id: EmploymentId::new(employment_id),
        period: PayPeriod::new(period_start, period_end)
            .expect("finalized_payroll CHECK: period_end is never before period_start"),
        pay_date,
        salt_version,
        payslip_template_version,
        employer_particulars,
        person_particulars,
        earning_lines: calculation.earning_lines,
        deductions: calculation.deductions,
        figures,
        replaces: replaces_finalized_payroll_id.map(FinalizedPayrollId::new),
        reversal,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three columns [`PayslipData`] cannot do without, named
    /// individually — a row missing only one of them still says which.
    #[test]
    fn missing_particulars_are_named_individually() {
        let id = FinalizedPayrollId::new("finalized-payroll-1");
        let err = PayrollAppError::PayslipParticularsNotFrozen {
            finalized_payroll_id: id,
            missing: vec!["EmployerParticulars", "PayslipTemplateVersion"],
        };

        assert_eq!(
            err.to_string(),
            "FinalizedPayroll finalized-payroll-1 cannot render a Payslip: EmployerParticulars \
             and PayslipTemplateVersion never froze on this row — it was finalized before Salt \
             began freezing them"
        );
    }
}
