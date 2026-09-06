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

/// The one snapshot layout this build knows how to decode: a
/// `PayrollCalculation` as `serde` serializes it today. A later layout gets
/// its own constant, its own match arm and its own decoder, while this one
/// stays for history — finalized snapshots are never migrated in place
/// (ADR-0012), so an old row keeps reading through the reader written for
/// it.
const SNAPSHOT_LAYOUT_V1: i32 = 1;

/// Decodes one frozen calculation by the layout named on its own row. A
/// version this build has no decoder for is refused rather than deserialized
/// as if it were current — the unit tests below pin both halves of that:
/// an unknown layout is never fed to `serde`, and the layout
/// [`crate::SNAPSHOT_SCHEMA_VERSION`] writes is always one of the layouts
/// decoded here.
fn calculation_from_snapshot(
    finalized_payroll_id: &FinalizedPayrollId,
    schema_version: i32,
    calculation_json: serde_json::Value,
) -> Result<PayrollCalculation, PayrollAppError> {
    match schema_version {
        SNAPSHOT_LAYOUT_V1 => serde_json::from_value(calculation_json).map_err(|_| {
            PayrollAppError::FinalizedPayrollSnapshotUnreadable {
                finalized_payroll_id: finalized_payroll_id.clone(),
                schema_version,
            }
        }),
        _ => Err(PayrollAppError::FinalizedPayrollSnapshotUnreadable {
            finalized_payroll_id: finalized_payroll_id.clone(),
            schema_version,
        }),
    }
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
    pub full_name: String,
    pub period: PayPeriod,
    pub pay_date: NaiveDate,
    pub figures: PayrollFigures,
    pub salt_version: String,
}

/// Reads one `FinalizedPayroll` back for display, scoped to `employer_id` in
/// SQL (ADR-0017). `payroll_calculation_json` is read and deserialized here
/// so `PayrollFigures::from_calculation` (crate-private) can extract the nine
/// figures — the deserialized value itself never leaves this function.
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
        String,
        NaiveDate,
        NaiveDate,
        NaiveDate,
        i32,
        serde_json::Value,
        String,
    );

    let row: Option<Row> = sqlx::query_as(
        "SELECT finalized_payroll.employment_id, person.full_name, finalized_payroll.period_start,
                finalized_payroll.period_end, payroll_run.pay_date,
                finalized_payroll.snapshot_schema_version,
                finalized_payroll.payroll_calculation_json, finalized_payroll.salt_version
         FROM finalized_payroll
         JOIN payroll_run ON payroll_run.id = finalized_payroll.payroll_run_id
         JOIN employment ON employment.id = finalized_payroll.employment_id
         JOIN person ON person.id = employment.person_id
                    AND person.employer_id = employment.employer_id
         WHERE finalized_payroll.id = $1::uuid AND finalized_payroll.employer_id = $2",
    )
    .bind(finalized_payroll_id.as_str())
    .bind(employer_id.as_str())
    .fetch_optional(db.pool())
    .await?;

    let (
        employment_id,
        full_name,
        period_start,
        period_end,
        pay_date,
        schema_version,
        calculation_json,
        salt_version,
    ) =
        row.ok_or_else(|| PayrollAppError::FinalizedPayrollNotFound(finalized_payroll_id.clone()))?;
    let calculation =
        calculation_from_snapshot(&finalized_payroll_id, schema_version, calculation_json)?;

    Ok(FinalizedPayrollDetail {
        id: finalized_payroll_id,
        employment_id: EmploymentId::new(employment_id),
        full_name,
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

    let snapshot: Option<(i32, serde_json::Value)> = sqlx::query_as(
        "SELECT snapshot_schema_version, payroll_calculation_json
         FROM finalized_payroll
         WHERE id = $1::uuid AND employer_id = $2",
    )
    .bind(finalized_payroll_id.as_str())
    .bind(employer_id.as_str())
    .fetch_optional(db.pool())
    .await?;

    let (schema_version, calculation_json) = snapshot
        .ok_or_else(|| PayrollAppError::FinalizedPayrollNotFound(finalized_payroll_id.clone()))?;
    let calculation =
        calculation_from_snapshot(&finalized_payroll_id, schema_version, calculation_json)?;

    Ok(FinalizedPayrollTraces {
        paye: calculation.paye.trace,
        employee_social_security: calculation.employee_social_security.trace,
        employer_social_security: calculation.employer_social_security.trace,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_snapshot_layout_is_refused_without_deserializing_it() {
        let id = FinalizedPayrollId::new("finalized-payroll-1");

        let result = calculation_from_snapshot(&id, 2, serde_json::json!({ "not": "v1" }));

        assert_eq!(
            result,
            Err(PayrollAppError::FinalizedPayrollSnapshotUnreadable {
                finalized_payroll_id: id,
                schema_version: 2,
            })
        );
    }

    /// Bumping [`crate::SNAPSHOT_SCHEMA_VERSION`] without adding the
    /// matching decoder arm above would make every payroll finalized by the
    /// new build unreadable by it — a 500 on the route whose whole job is
    /// explaining a past month. The compiler cannot see that link, so this
    /// test is what holds it.
    #[test]
    fn the_layout_this_build_writes_is_a_layout_this_build_can_read() {
        assert_eq!(
            crate::SNAPSHOT_SCHEMA_VERSION,
            SNAPSHOT_LAYOUT_V1,
            "a new snapshot layout needs its own arm in calculation_from_snapshot"
        );
    }
}
