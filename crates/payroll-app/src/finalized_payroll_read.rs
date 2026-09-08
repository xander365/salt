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

/// Decodes one frozen calculation by the layout named on its own row. A
/// version this build has no decoder for is refused rather than deserialized
/// as if it were current — the unit tests below pin both halves of that: an
/// unknown layout is never fed to `serde`, and the layout
/// [`crate::SNAPSHOT_SCHEMA_VERSION`] writes is always one of the layouts
/// decoded here.
///
/// Every version in [`crate::finalize::KNOWN_JSON_SNAPSHOT_VERSIONS`] shares
/// this one decoder: issue #73's version 2 added sibling columns beside
/// `payroll_calculation_json`, never reshaped the blob itself, so 1 and 2
/// both decode through it. A version that does reshape the blob gets its own
/// arm here and its own entry in that list, while this decoder stays for the
/// versions written before it — finalized snapshots are never migrated in
/// place (ADR-0012).
fn calculation_from_snapshot(
    finalized_payroll_id: &FinalizedPayrollId,
    schema_version: i32,
    calculation_json: serde_json::Value,
) -> Result<PayrollCalculation, PayrollAppError> {
    if !crate::finalize::KNOWN_JSON_SNAPSHOT_VERSIONS.contains(&schema_version) {
        return Err(PayrollAppError::FinalizedPayrollSnapshotUnreadable {
            finalized_payroll_id: finalized_payroll_id.clone(),
            schema_version,
        });
    }
    serde_json::from_value(calculation_json).map_err(|_| {
        PayrollAppError::FinalizedPayrollSnapshotUnreadable {
            finalized_payroll_id: finalized_payroll_id.clone(),
            schema_version,
        }
    })
}

/// Decodes one frozen particulars column, which is absent (`None`) on every
/// row finalized before issue #73 and, for
/// [`FinalizedEmployerParticulars`], on a row whose Employer had recorded
/// nothing at finalize time.
///
/// A blob that is present but does not decode is refused as
/// [`PayrollAppError::FinalizedPayrollSnapshotUnreadable`], exactly as an
/// undecodable `payroll_calculation_json` already is — never a panic. The
/// column is written only by `finalize_payroll_run` and never updated in
/// place (§6.2 revokes `UPDATE`), so this cannot happen to a row this build
/// wrote; it can happen to a row written by a *later* build whose version
/// this one does not know, and that case is already the one
/// [`calculation_from_snapshot`] answers with this same error rather than a
/// 500 on the route whose whole job is explaining a past month.
fn particulars_from_snapshot<T: serde::de::DeserializeOwned>(
    finalized_payroll_id: &FinalizedPayrollId,
    schema_version: i32,
    particulars_json: Option<serde_json::Value>,
) -> Result<Option<T>, PayrollAppError> {
    particulars_json
        .map(|value| {
            serde_json::from_value(value).map_err(|_| {
                PayrollAppError::FinalizedPayrollSnapshotUnreadable {
                    finalized_payroll_id: finalized_payroll_id.clone(),
                    schema_version,
                }
            })
        })
        .transpose()
}

/// The frozen `EmployerParticulars` on one `FinalizedPayroll` (issue #73,
/// CONTEXT.md's own glossary entry). `None` on
/// [`FinalizedPayrollDetail::employer_particulars`] means either this
/// Employer had never recorded particulars at finalize time, or this row
/// finalized before issue #73 shipped at all — the two read back
/// identically, which is the point: presence is what a reader tests, never
/// `snapshot_schema_version` (see `crate::finalize`'s own module doc).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct FinalizedEmployerParticulars {
    pub registered_name: String,
    pub address_line1: String,
    pub address_line2: Option<String>,
    pub city: String,
    pub postal_code: Option<String>,
    pub income_tax_number: Option<String>,
    pub social_security_number: Option<String>,
}

/// The frozen `PersonParticulars` on one `FinalizedPayroll` (issue #73). When
/// this is `Some`, `full_name` is never absent — every Person has one — but
/// `None` on [`FinalizedPayrollDetail::person_particulars`] itself still
/// means what it means for [`FinalizedEmployerParticulars`]: nothing froze
/// here, because this row predates issue #73.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct FinalizedPersonParticulars {
    pub full_name: String,
    pub identity_number: Option<String>,
    pub address_line1: Option<String>,
    pub address_line2: Option<String>,
    pub city: Option<String>,
    pub postal_code: Option<String>,
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
    /// The frozen `EmployerParticulars` (issue #73) — `None` when nothing
    /// froze here (see [`FinalizedEmployerParticulars`]'s own docs).
    pub employer_particulars: Option<FinalizedEmployerParticulars>,
    /// The frozen `PersonParticulars` (issue #73), including the `full_name`
    /// on record at finalize time — `None` when nothing froze here (see
    /// [`FinalizedPersonParticulars`]'s own docs).
    pub person_particulars: Option<FinalizedPersonParticulars>,
    /// The frozen `PayslipTemplateVersion` (issue #73) — `None` under the
    /// same "nothing froze here" condition as the two particulars above.
    pub payslip_template_version: Option<String>,
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
///
/// `full_name` prefers the frozen `person_particulars_json`'s own
/// `full_name` (issue #73) over the live `person.full_name` join: a row that
/// froze one has it forever, so a later `correct_person_full_name` can never
/// rewrite what this reads back — exactly D22's own promise. The live join
/// stays, and is fallen back to, only for a row that predates issue #73 and
/// so never froze a name to prefer.
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
        Option<serde_json::Value>,
        Option<serde_json::Value>,
        Option<String>,
    );

    let row: Option<Row> = sqlx::query_as(
        "SELECT finalized_payroll.employment_id, person.full_name, finalized_payroll.period_start,
                finalized_payroll.period_end, payroll_run.pay_date,
                finalized_payroll.snapshot_schema_version,
                finalized_payroll.payroll_calculation_json, finalized_payroll.salt_version,
                finalized_payroll.employer_particulars_json,
                finalized_payroll.person_particulars_json,
                finalized_payroll.payslip_template_version
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
        live_full_name,
        period_start,
        period_end,
        pay_date,
        schema_version,
        calculation_json,
        salt_version,
        employer_particulars_json,
        person_particulars_json,
        payslip_template_version,
    ) =
        row.ok_or_else(|| PayrollAppError::FinalizedPayrollNotFound(finalized_payroll_id.clone()))?;
    let calculation =
        calculation_from_snapshot(&finalized_payroll_id, schema_version, calculation_json)?;

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
    let full_name = person_particulars
        .as_ref()
        .map(|particulars| particulars.full_name.clone())
        .unwrap_or(live_full_name);

    Ok(FinalizedPayrollDetail {
        id: finalized_payroll_id,
        employment_id: EmploymentId::new(employment_id),
        full_name,
        period: PayPeriod::new(period_start, period_end)
            .expect("finalized_payroll CHECK: period_end is never before period_start"),
        pay_date,
        figures: PayrollFigures::from_calculation(&calculation),
        salt_version,
        employer_particulars,
        person_particulars,
        payslip_template_version,
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

        let result = calculation_from_snapshot(&id, 99, serde_json::json!({ "not": "v1" }));

        assert_eq!(
            result,
            Err(PayrollAppError::FinalizedPayrollSnapshotUnreadable {
                finalized_payroll_id: id,
                schema_version: 99,
            })
        );
    }

    /// An absent column is the ordinary case, not a failure: a row finalized
    /// before issue #73, or one whose Employer had recorded nothing.
    #[test]
    fn an_absent_particulars_column_reads_back_as_nothing_frozen() {
        let id = FinalizedPayrollId::new("finalized-payroll-1");

        let decoded: Option<FinalizedEmployerParticulars> =
            particulars_from_snapshot(&id, 2, None).unwrap();

        assert_eq!(decoded, None);
    }

    /// A present-but-undecodable blob is refused with the same error an
    /// undecodable calculation snapshot gets, never a panic — a 500 with a
    /// backtrace is not how this route explains a past month.
    #[test]
    fn a_particulars_blob_this_build_cannot_decode_is_refused_rather_than_panicking() {
        let id = FinalizedPayrollId::new("finalized-payroll-1");

        let result: Result<Option<FinalizedPersonParticulars>, _> =
            particulars_from_snapshot(&id, 3, Some(serde_json::json!({ "not": "particulars" })));

        assert_eq!(
            result,
            Err(PayrollAppError::FinalizedPayrollSnapshotUnreadable {
                finalized_payroll_id: id,
                schema_version: 3,
            })
        );
    }

    /// Bumping [`crate::SNAPSHOT_SCHEMA_VERSION`] without adding it to
    /// `KNOWN_JSON_SNAPSHOT_VERSIONS` would make every payroll finalized by
    /// the new build unreadable by it — a 500 on the route whose whole job
    /// is explaining a past month. The compiler cannot see that link, so
    /// this test is what holds it.
    #[test]
    fn the_layout_this_build_writes_is_a_known_layout() {
        assert!(
            crate::finalize::KNOWN_JSON_SNAPSHOT_VERSIONS.contains(&crate::SNAPSHOT_SCHEMA_VERSION),
            "a new snapshot layout needs its own entry in KNOWN_JSON_SNAPSHOT_VERSIONS"
        );
    }
}
