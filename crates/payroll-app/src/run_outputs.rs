//! `GetPayrollRegister` and `GetPaymentSummary` (issue #83, parent #70
//! §D-9, §D-10): two read-only views over a finalized PayrollRun's
//! `FinalizedPayroll` rows. Both share one private row reader
//! ([`read_finalized_payroll_rows`]) so liveness, the frozen-name-first rule
//! and the figures can never drift apart between the two projections — the
//! Register shows every row this run produced; the PaymentSummary shows only
//! the ones still Live.
//!
//! Liveness, replacement and the name rule are read exactly as
//! [`crate::payslip::get_payslip_data`] already reads them for one row at a
//! time: a row is Live when `live_finalized_payroll` names its id, Reversed
//! when `reversal` names it instead — the two are mutually exclusive, since
//! `reverse_finalized_payroll` deletes the liveness row in the same
//! transaction it inserts the `Reversal` — and it is a Replacement when its
//! own `replaces_finalized_payroll_id` is set. A name prefers the frozen
//! `person_particulars_json.full_name`, falling back to the live
//! `person.full_name` only for a row that froze none — the same rule
//! [`crate::get_finalized_payroll_detail`] already follows.
//!
//! Both refuse a run that has not finalized
//! ([`PayrollAppError::PayrollRunNotFinalized`]): a Draft or Calculated run
//! has no `FinalizedPayroll` row for either read model to show yet.

use chrono::{DateTime, NaiveDate, Utc};
use payroll::{EmployerId, EmploymentId, Money, PayPeriod, PayrollError};

use crate::database::SaltDatabase;
use crate::error::PayrollAppError;
use crate::finalize::FinalizedPayrollId;
use crate::finalized_payroll_read::{
    FinalizedPersonParticulars, calculation_from_snapshot, particulars_from_snapshot,
};
use crate::payroll_run::{PayrollFigures, PayrollRunId, RunKind, RunStatus, parse_payroll_run_id};

/// Whether one row is still Live, or has been Reversed — CONTEXT.md's own
/// `Reversal`/`Replacement` entries, restated as a type instead of nullable
/// columns a reader must reconcile themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinalizedPayrollLiveness {
    Live,
    Reversed {
        reason: String,
        reversed_at: DateTime<Utc>,
        replaced_by: Option<FinalizedPayrollId>,
    },
}

/// One row of a [`PayrollRegister`]: every `FinalizedPayroll` this run
/// produced, whatever its liveness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayrollRegisterRow {
    pub finalized_payroll_id: FinalizedPayrollId,
    pub employment_id: EmploymentId,
    pub full_name: String,
    pub figures: PayrollFigures,
    pub liveness: FinalizedPayrollLiveness,
    /// Present when this row is itself a Replacement (CONTEXT.md): the
    /// `FinalizedPayroll` it replaces.
    pub replaces: Option<FinalizedPayrollId>,
}

/// The same eleven money fields [`PayrollFigures`] carries, summed across
/// however many rows a [`PayrollRegister`] totals — "as finalized" over
/// every row, "still live" over the Live ones only (§D-9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayrollRegisterTotals {
    pub basic_pay: Money,
    pub taxable_allowances: Money,
    pub overtime: Money,
    pub gross: Money,
    pub taxable_remuneration: Money,
    pub paye: Money,
    pub employee_social_security: Money,
    pub employer_social_security: Money,
    pub medical_aid_premium: Money,
    pub total_deductions: Money,
    pub net_pay: Money,
}

impl PayrollRegisterTotals {
    /// Sums each field with `Money`'s own checked API throughout (never
    /// `f64`, INV-001). A run whose total would overflow a `Money` is
    /// refused rather than shown wrong — the same `PayrollError::AmountOverflow`
    /// path [`crate::build_year_to_date_context`] already answers with for a
    /// TaxYear's running total.
    fn sum<'a>(
        figures: impl Iterator<Item = &'a PayrollFigures> + Clone,
    ) -> Result<Self, PayrollAppError> {
        let overflow = || PayrollAppError::from(PayrollError::AmountOverflow);
        Ok(PayrollRegisterTotals {
            basic_pay: Money::checked_sum(figures.clone().map(|f| f.basic_pay))
                .map_err(|_| overflow())?,
            taxable_allowances: Money::checked_sum(figures.clone().map(|f| f.taxable_allowances))
                .map_err(|_| overflow())?,
            overtime: Money::checked_sum(figures.clone().map(|f| f.overtime))
                .map_err(|_| overflow())?,
            gross: Money::checked_sum(figures.clone().map(|f| f.gross)).map_err(|_| overflow())?,
            taxable_remuneration: Money::checked_sum(
                figures.clone().map(|f| f.taxable_remuneration),
            )
            .map_err(|_| overflow())?,
            paye: Money::checked_sum(figures.clone().map(|f| f.paye)).map_err(|_| overflow())?,
            employee_social_security: Money::checked_sum(
                figures.clone().map(|f| f.employee_social_security),
            )
            .map_err(|_| overflow())?,
            employer_social_security: Money::checked_sum(
                figures.clone().map(|f| f.employer_social_security),
            )
            .map_err(|_| overflow())?,
            medical_aid_premium: Money::checked_sum(figures.clone().map(|f| f.medical_aid_premium))
                .map_err(|_| overflow())?,
            total_deductions: Money::checked_sum(figures.clone().map(|f| f.total_deductions))
                .map_err(|_| overflow())?,
            net_pay: Money::checked_sum(figures.map(|f| f.net_pay)).map_err(|_| overflow())?,
        })
    }
}

/// Every Employment in a finalized PayrollRun, with each row's liveness and
/// two totals — "as finalized" and "still live" (§D-9, issue #83).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayrollRegister {
    pub payroll_run_id: PayrollRunId,
    pub kind: RunKind,
    pub period: PayPeriod,
    pub pay_date: NaiveDate,
    pub rows: Vec<PayrollRegisterRow>,
    pub total_as_finalized: PayrollRegisterTotals,
    pub total_still_live: PayrollRegisterTotals,
}

/// One row of a [`PaymentSummary`]: a Live record only (a Reversed one is
/// excluded, never listed at zero).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentSummaryRow {
    pub finalized_payroll_id: FinalizedPayrollId,
    pub employment_id: EmploymentId,
    pub full_name: String,
    pub net_pay: Money,
    /// Some when this row is itself a Replacement. The UI/PDF must then say
    /// the original may already have been paid and a human decides the
    /// actual transfer (§D-10, issue #83 acceptance criterion 4).
    pub replaces: Option<FinalizedPayrollId>,
}

/// Names and net pay for a finalized PayrollRun's Live records only, stating
/// how many rows were excluded as reversed (§D-10, issue #83). Producing
/// this means nobody has been paid — Salt writes no bank file and computes
/// no transfer (acceptance criterion 3, ADR the "no CSV, no bank file" rule
/// restates).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentSummary {
    pub payroll_run_id: PayrollRunId,
    pub kind: RunKind,
    pub period: PayPeriod,
    pub pay_date: NaiveDate,
    pub rows: Vec<PaymentSummaryRow>,
    pub excluded_reversed_count: usize,
    pub total_net_pay: Money,
}

/// A `payroll_run` row's header fields, read once per call and shared by
/// both public use cases below.
struct RunHeader {
    kind: RunKind,
    period: PayPeriod,
    pay_date: NaiveDate,
}

/// Reads `payroll_run_id`'s header, scoped to `employer_id` in SQL
/// (ADR-0017) — unknown and cross-Employer both answer
/// [`PayrollAppError::PayrollRunNotFound`]. Refused as
/// [`PayrollAppError::PayrollRunNotFinalized`] for any other status: a
/// Draft or Calculated run has no `FinalizedPayroll` row for either read
/// model in this module to show.
async fn read_run_header(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    employer_id: &EmployerId,
    payroll_run_id: &PayrollRunId,
) -> Result<RunHeader, PayrollAppError> {
    type Row = (NaiveDate, NaiveDate, NaiveDate, String, String);

    let run: Option<Row> = sqlx::query_as(
        "SELECT period_start, period_end, pay_date, status, kind
         FROM payroll_run
         WHERE id = $1::uuid AND employer_id = $2",
    )
    .bind(payroll_run_id.as_str())
    .bind(employer_id.as_str())
    .fetch_optional(&mut **tx)
    .await?;

    let (period_start, period_end, pay_date, status, kind) =
        run.ok_or_else(|| PayrollAppError::PayrollRunNotFound(payroll_run_id.clone()))?;

    if RunStatus::from_column(&status) != RunStatus::Finalized {
        return Err(PayrollAppError::PayrollRunNotFinalized(
            payroll_run_id.clone(),
        ));
    }

    Ok(RunHeader {
        kind: RunKind::from_column(&kind),
        period: PayPeriod::new(period_start, period_end)
            .expect("payroll_run CHECK: period_end is never before period_start"),
        pay_date,
    })
}

/// One `FinalizedPayroll` row of `payroll_run_id`, before it is projected
/// into either a [`PayrollRegisterRow`] or a [`PaymentSummaryRow`].
struct FinalizedPayrollRow {
    finalized_payroll_id: FinalizedPayrollId,
    employment_id: EmploymentId,
    full_name: String,
    figures: PayrollFigures,
    liveness: FinalizedPayrollLiveness,
    replaces: Option<FinalizedPayrollId>,
}

/// Reads every `FinalizedPayroll` row `payroll_run_id` produced, in
/// `ORDER BY employment_id` — the one member order batch payslips, the
/// register and the summary all share (README.md). One query, joining
/// liveness, reversal and replacement exactly as
/// [`crate::payslip::get_payslip_data`] does for a single row.
///
/// Employer scope is not repeated here: `payroll_run_id` was already
/// resolved against `employer_id` by [`read_run_header`] in the same
/// transaction, and every row here belongs to that one run.
async fn read_finalized_payroll_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    payroll_run_id: &PayrollRunId,
) -> Result<Vec<FinalizedPayrollRow>, PayrollAppError> {
    type Row = (
        String,                    // finalized_payroll.id
        String,                    // employment_id
        String,                    // person.full_name (live)
        i32,                       // snapshot_schema_version
        serde_json::Value,         // payroll_calculation_json
        Option<serde_json::Value>, // person_particulars_json
        Option<String>,            // replaces_finalized_payroll_id
        bool,                      // is_live
        Option<String>,            // reversal.reason
        Option<DateTime<Utc>>,     // reversal.reversed_at
        Option<String>,            // the id of whatever replaces this row
    );

    let rows: Vec<Row> = sqlx::query_as(
        "SELECT finalized_payroll.id::text,
                finalized_payroll.employment_id,
                person.full_name,
                finalized_payroll.snapshot_schema_version,
                finalized_payroll.payroll_calculation_json,
                finalized_payroll.person_particulars_json,
                finalized_payroll.replaces_finalized_payroll_id::text,
                live.finalized_payroll_id IS NOT NULL,
                reversal.reason,
                reversal.reversed_at,
                replacement.id::text
         FROM finalized_payroll
         JOIN employment ON employment.id = finalized_payroll.employment_id
         JOIN person ON person.id = employment.person_id
                    AND person.employer_id = employment.employer_id
         LEFT JOIN live_finalized_payroll AS live
                ON live.finalized_payroll_id = finalized_payroll.id
         LEFT JOIN reversal ON reversal.finalized_payroll_id = finalized_payroll.id
         LEFT JOIN finalized_payroll AS replacement
                ON replacement.replaces_finalized_payroll_id = finalized_payroll.id
         WHERE finalized_payroll.payroll_run_id = $1::uuid
         ORDER BY finalized_payroll.employment_id",
    )
    .bind(payroll_run_id.as_str())
    .fetch_all(&mut **tx)
    .await?;

    rows.into_iter()
        .map(
            |(
                finalized_payroll_id,
                employment_id,
                live_full_name,
                schema_version,
                calculation_json,
                person_particulars_json,
                replaces_finalized_payroll_id,
                is_live,
                reversal_reason,
                reversal_reversed_at,
                replacement_id,
            )| {
                let finalized_payroll_id = FinalizedPayrollId::new(finalized_payroll_id);

                let calculation = calculation_from_snapshot(
                    &finalized_payroll_id,
                    schema_version,
                    calculation_json,
                )?;
                let person_particulars: Option<FinalizedPersonParticulars> =
                    particulars_from_snapshot(
                        &finalized_payroll_id,
                        schema_version,
                        person_particulars_json,
                    )?;
                let full_name = person_particulars
                    .map(|particulars| particulars.full_name)
                    .unwrap_or(live_full_name);

                let liveness = match reversal_reason {
                    Some(reason) => FinalizedPayrollLiveness::Reversed {
                        reason,
                        reversed_at: reversal_reversed_at.expect(
                            "reversal.reason is Some iff reversal.reversed_at is, from the \
                             same row",
                        ),
                        replaced_by: replacement_id.map(FinalizedPayrollId::new),
                    },
                    None => {
                        assert!(
                            is_live,
                            "a FinalizedPayroll row with no Reversal is always Live: \
                             reverse_finalized_payroll deletes the liveness row in the same \
                             transaction it inserts the Reversal, so the two never disagree"
                        );
                        FinalizedPayrollLiveness::Live
                    }
                };

                Ok(FinalizedPayrollRow {
                    finalized_payroll_id,
                    employment_id: EmploymentId::new(employment_id),
                    full_name,
                    figures: PayrollFigures::from_calculation(&calculation),
                    liveness,
                    replaces: replaces_finalized_payroll_id.map(FinalizedPayrollId::new),
                })
            },
        )
        .collect()
}

/// Every Employment a finalized PayrollRun paid, with each row's liveness
/// and the run's two totals (§D-9, issue #83). Refused as
/// [`PayrollAppError::PayrollRunNotFound`] for an unknown or cross-Employer
/// run id (ADR-0017), and as [`PayrollAppError::PayrollRunNotFinalized`] for
/// a run that has not yet finalized.
pub async fn get_payroll_register(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    payroll_run_id: &str,
) -> Result<PayrollRegister, PayrollAppError> {
    let payroll_run_id = parse_payroll_run_id(payroll_run_id)?;
    let mut tx = db.pool().begin().await?;

    let header = read_run_header(&mut tx, employer_id, &payroll_run_id).await?;
    let rows = read_finalized_payroll_rows(&mut tx, &payroll_run_id).await?;
    tx.commit().await?;

    let live_figures: Vec<PayrollFigures> = rows
        .iter()
        .filter(|row| matches!(row.liveness, FinalizedPayrollLiveness::Live))
        .map(|row| row.figures)
        .collect();
    let total_as_finalized = PayrollRegisterTotals::sum(rows.iter().map(|row| &row.figures))?;
    let total_still_live = PayrollRegisterTotals::sum(live_figures.iter())?;

    Ok(PayrollRegister {
        payroll_run_id,
        kind: header.kind,
        period: header.period,
        pay_date: header.pay_date,
        rows: rows
            .into_iter()
            .map(|row| PayrollRegisterRow {
                finalized_payroll_id: row.finalized_payroll_id,
                employment_id: row.employment_id,
                full_name: row.full_name,
                figures: row.figures,
                liveness: row.liveness,
                replaces: row.replaces,
            })
            .collect(),
        total_as_finalized,
        total_still_live,
    })
}

/// Names and net pay for a finalized PayrollRun's Live records only (§D-10,
/// issue #83) — the same refusals as [`get_payroll_register`], for the same
/// run.
pub async fn get_payment_summary(
    db: &SaltDatabase,
    employer_id: &EmployerId,
    payroll_run_id: &str,
) -> Result<PaymentSummary, PayrollAppError> {
    let payroll_run_id = parse_payroll_run_id(payroll_run_id)?;
    let mut tx = db.pool().begin().await?;

    let header = read_run_header(&mut tx, employer_id, &payroll_run_id).await?;
    let rows = read_finalized_payroll_rows(&mut tx, &payroll_run_id).await?;
    tx.commit().await?;

    let excluded_reversed_count = rows
        .iter()
        .filter(|row| matches!(row.liveness, FinalizedPayrollLiveness::Reversed { .. }))
        .count();

    let rows: Vec<PaymentSummaryRow> = rows
        .into_iter()
        .filter(|row| matches!(row.liveness, FinalizedPayrollLiveness::Live))
        .map(|row| PaymentSummaryRow {
            finalized_payroll_id: row.finalized_payroll_id,
            employment_id: row.employment_id,
            full_name: row.full_name,
            net_pay: row.figures.net_pay,
            replaces: row.replaces,
        })
        .collect();

    let total_net_pay = Money::checked_sum(rows.iter().map(|row| row.net_pay))
        .map_err(|_| PayrollAppError::from(PayrollError::AmountOverflow))?;

    Ok(PaymentSummary {
        payroll_run_id,
        kind: header.kind,
        period: header.period,
        pay_date: header.pay_date,
        rows,
        excluded_reversed_count,
        total_net_pay,
    })
}
