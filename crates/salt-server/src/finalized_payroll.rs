//! `GET /api/employers/{e}/finalized-payroll/{f}` and `GET
//! /api/employers/{e}/finalized-payroll/{f}/traces` (issue #57, parent #49
//! Spec 2 of 3). An Operator opens a payroll that has already been
//! finalized and reads it back — the ten figures, the period, the pay
//! date and the version of Salt that produced them — so a question about a
//! past month can be answered. The PAYE and social security workings are on
//! their own endpoint, deliberately never inlined into the detail response
//! (§0.29).
//!
//! Both DTOs below are hand-written (§0.29): neither ever serializes
//! `payroll-app`'s stored `payroll_input_json`, `payroll_rules_json` or
//! `payroll_calculation_json` straight onto the wire. `figures_to_dto`
//! reuses [`crate::payroll_runs`]'s own DTO — the same ten names a working
//! run's `GET`/`calculate` response already carries — so a figure looks the
//! same whether the run is `Calculated` or already `Finalized`.
//!
//! Every handler here takes its `EmployerId` from
//! [`AuthorizedEmployerContext`] and never from the path (ADR-0017); the
//! `employer_id` scoping itself happens in `payroll-app`'s own SQL
//! (§0.30), so a `finalized_payroll_id` belonging to another Employer reads
//! back as `finalized_payroll_not_found` — the same 404 an unknown id gets.
//!
//! Both routes are readable by a `PayrollOperator` as well as an Owner
//! (§0.6): neither calls `AuthorizedEmployerContext::require_role`.

use axum::Json;
use axum::extract::{Path, State};
use chrono::NaiveDate;
use payroll::{EmployerId, PayeTrace, SaltPolicyStatus, SscClamp, SscTrace};
use payroll_app::OvertimeLineTrace;
use rust_decimal::Decimal;
use serde::Serialize;

use crate::authorized_employer::AuthorizedEmployerContext;
use crate::employment_facts::PayPeriodDto;
use crate::error::ApiError;
use crate::payroll_runs::{FiguresDto, figures_to_dto};
use crate::state::AppState;

/// The frozen `EmployerParticulars` on one `FinalizedPayroll` (issue #73):
/// `null` on the wire means either the Employer had never recorded
/// particulars at finalize time, or this row predates issue #73 and never
/// froze one at all — the two are indistinguishable on purpose (see
/// `payroll_app::FinalizedEmployerParticulars`'s own docs).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FrozenEmployerParticularsDto {
    registered_name: String,
    address_line1: String,
    address_line2: Option<String>,
    city: String,
    postal_code: Option<String>,
    income_tax_number: Option<String>,
    social_security_number: Option<String>,
}

impl From<payroll_app::FinalizedEmployerParticulars> for FrozenEmployerParticularsDto {
    fn from(particulars: payroll_app::FinalizedEmployerParticulars) -> Self {
        Self {
            registered_name: particulars.registered_name,
            address_line1: particulars.address_line1,
            address_line2: particulars.address_line2,
            city: particulars.city,
            postal_code: particulars.postal_code,
            income_tax_number: particulars.income_tax_number,
            social_security_number: particulars.social_security_number,
        }
    }
}

/// The frozen `PersonParticulars` on one `FinalizedPayroll` (issue #73):
/// `fullName` is never absent when this is present at all — see
/// `payroll_app::FinalizedPersonParticulars`'s own docs for what `null` on
/// the whole object means.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FrozenPersonParticularsDto {
    full_name: String,
    identity_number: Option<String>,
    address_line1: Option<String>,
    address_line2: Option<String>,
    city: Option<String>,
    postal_code: Option<String>,
}

impl From<payroll_app::FinalizedPersonParticulars> for FrozenPersonParticularsDto {
    fn from(particulars: payroll_app::FinalizedPersonParticulars) -> Self {
        Self {
            full_name: particulars.full_name,
            identity_number: particulars.identity_number,
            address_line1: particulars.address_line1,
            address_line2: particulars.address_line2,
            city: particulars.city,
            postal_code: particulars.postal_code,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FinalizedPayrollDetailResponse {
    finalized_payroll_id: String,
    employment_id: String,
    full_name: String,
    period: PayPeriodDto,
    pay_date: NaiveDate,
    figures: FiguresDto,
    salt_version: String,
    employer_particulars: Option<FrozenEmployerParticularsDto>,
    person_particulars: Option<FrozenPersonParticularsDto>,
    payslip_template_version: Option<String>,
}

/// `GET /api/employers/{e}/finalized-payroll/{f}`: the ten figures, the
/// period, the pay date and the `SaltVersion` of one finalized payroll. An
/// id belonging to another Employer is 404, indistinguishable from an
/// unknown one (ADR-0017) — `payroll_app::get_finalized_payroll_detail`
/// filters on `employer_id` in its own SQL, so this handler checks nothing
/// itself before calling it.
pub(crate) async fn get_finalized_payroll(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, finalized_payroll_id)): Path<(String, String)>,
) -> Result<Json<FinalizedPayrollDetailResponse>, ApiError> {
    let employer_id = EmployerId::new(context.employer_id().as_str());

    let detail =
        payroll_app::get_finalized_payroll_detail(state.db(), &employer_id, &finalized_payroll_id)
            .await?;

    Ok(Json(FinalizedPayrollDetailResponse {
        finalized_payroll_id: detail.id.to_string(),
        employment_id: detail.employment_id.to_string(),
        full_name: detail.full_name,
        period: PayPeriodDto::from(detail.period),
        pay_date: detail.pay_date,
        figures: figures_to_dto(detail.figures),
        salt_version: detail.salt_version,
        employer_particulars: detail.employer_particulars.map(Into::into),
        person_particulars: detail.person_particulars.map(Into::into),
        payslip_template_version: detail.payslip_template_version,
    }))
}

/// One PAYE band this period's cumulative tax crossed, and how much it
/// contributed. Not cents-exact (INV-001 covers `Money`, not the
/// intermediate decimal arithmetic bands are computed from), so `threshold`,
/// `rate` and `tax` are strings — never a JSON float.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BandContributionDto {
    threshold: String,
    rate: String,
    tax: String,
}

/// The year-to-date figures PAYE was derived from, hand-mapped from
/// [`payroll::PayeTrace`] so its shape is a deliberate wire contract, not
/// whatever `#[derive(Serialize)]` on the domain type would happen to emit.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PayeTraceDto {
    prior_taxable_remuneration_cents: i64,
    prior_paye_cents: i64,
    this_period_taxable_remuneration_cents: i64,
    year_to_date_taxable_remuneration_cents: i64,
    year_to_date_tax_owed: String,
    bands_applied: Vec<BandContributionDto>,
    periods_elapsed: u8,
}

fn paye_trace_to_dto(trace: PayeTrace) -> PayeTraceDto {
    PayeTraceDto {
        prior_taxable_remuneration_cents: trace.prior_taxable_remuneration.cents(),
        prior_paye_cents: trace.prior_paye.cents(),
        this_period_taxable_remuneration_cents: trace.this_period_taxable_remuneration.cents(),
        year_to_date_taxable_remuneration_cents: trace.year_to_date_taxable_remuneration.cents(),
        year_to_date_tax_owed: decimal_to_dto(trace.year_to_date_tax_owed),
        bands_applied: trace
            .bands_applied
            .into_iter()
            .map(|band| BandContributionDto {
                threshold: decimal_to_dto(band.threshold),
                rate: decimal_to_dto(band.rate),
                tax: decimal_to_dto(band.tax),
            })
            .collect(),
        periods_elapsed: trace.periods_elapsed.get(),
    }
}

fn decimal_to_dto(value: Decimal) -> String {
    value.to_string()
}

/// `basic_pay_cents` clamped to `floor_cents`/`ceiling_cents` and the rate
/// applied — the base, rate, and any floor or ceiling behind one social
/// security figure, hand-mapped from [`payroll::SscTrace`] for the same
/// reason [`PayeTraceDto`] is.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SscTraceDto {
    basic_pay_cents: i64,
    base_cents: i64,
    clamp: &'static str,
    rate: String,
    floor_cents: i64,
    ceiling_cents: i64,
}

fn clamp_str(clamp: SscClamp) -> &'static str {
    match clamp {
        SscClamp::None => "none",
        SscClamp::Floor => "floor",
        SscClamp::Ceiling => "ceiling",
    }
}

fn ssc_trace_to_dto(trace: SscTrace) -> SscTraceDto {
    SscTraceDto {
        basic_pay_cents: trace.basic_pay.cents(),
        base_cents: trace.base.cents(),
        clamp: clamp_str(trace.clamp),
        rate: decimal_to_dto(trace.rate),
        floor_cents: trace.floor.cents(),
        ceiling_cents: trace.ceiling.cents(),
    }
}

/// One overtime line's workings, hand-mapped from
/// [`payroll_app::OvertimeLineTrace`] for the same reason [`PayeTraceDto`]
/// is. Every figure an Operator needs to redo `BasicPay x 12 / 52 /
/// OrdinaryHours x hours x multiplier` by hand, plus the stamp saying who
/// chose the divisor.
///
/// `derivedHourlyRate` is the exact unrounded rate as a decimal string, not
/// cents: it is intermediate arithmetic and is never rounded (ADR-0022).
/// The one rounding on the line produced `amountCents`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OvertimeTraceDto {
    amount_cents: i64,
    label: Option<String>,
    basic_pay_cents: i64,
    ordinary_hours: String,
    months_per_year: String,
    weeks_per_year: String,
    derived_hourly_rate: String,
    hours: String,
    multiplier: String,
    /// `"SC-OPEN-6"` — the conformance record's own reference.
    policy_reference: &'static str,
    /// `"needs_confirmation"`. A wire *code*, like `clamp` above: the words
    /// an Operator reads are the screen's, never a sentence this crate
    /// writes. What it must never be rendered as is law.
    policy_status: &'static str,
}

fn policy_status_str(status: SaltPolicyStatus) -> &'static str {
    match status {
        SaltPolicyStatus::NeedsConfirmation => "needs_confirmation",
    }
}

fn overtime_trace_to_dto(line: OvertimeLineTrace) -> OvertimeTraceDto {
    OvertimeTraceDto {
        amount_cents: line.amount.cents(),
        label: line.label.map(|label| label.to_string()),
        basic_pay_cents: line.trace.basic_pay.cents(),
        ordinary_hours: decimal_to_dto(line.trace.ordinary_hours.as_decimal()),
        months_per_year: decimal_to_dto(line.trace.months_per_year),
        weeks_per_year: decimal_to_dto(line.trace.weeks_per_year),
        derived_hourly_rate: decimal_to_dto(line.trace.derived_hourly_rate),
        hours: decimal_to_dto(line.trace.hours.as_decimal()),
        multiplier: decimal_to_dto(line.trace.multiplier.as_decimal()),
        policy_reference: line.trace.policy.id.reference(),
        policy_status: policy_status_str(line.trace.policy.status),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FinalizedPayrollTracesResponse {
    paye: PayeTraceDto,
    employee_ssc: SscTraceDto,
    employer_ssc: SscTraceDto,
    /// One entry per overtime line, empty for a salary-only payroll.
    overtime: Vec<OvertimeTraceDto>,
}

/// `GET /api/employers/{e}/finalized-payroll/{f}/traces`: the PAYE and
/// social security workings behind one finalized payroll's figures — a
/// separate endpoint from [`get_finalized_payroll`] (§0.29), never inlined
/// into it. Same scoping and same 404 behaviour as the detail route above.
pub(crate) async fn get_finalized_payroll_traces(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, finalized_payroll_id)): Path<(String, String)>,
) -> Result<Json<FinalizedPayrollTracesResponse>, ApiError> {
    let employer_id = EmployerId::new(context.employer_id().as_str());

    let traces =
        payroll_app::get_finalized_payroll_traces(state.db(), &employer_id, &finalized_payroll_id)
            .await?;

    Ok(Json(FinalizedPayrollTracesResponse {
        paye: paye_trace_to_dto(traces.paye),
        employee_ssc: ssc_trace_to_dto(traces.employee_social_security),
        employer_ssc: ssc_trace_to_dto(traces.employer_social_security),
        overtime: traces
            .overtime
            .into_iter()
            .map(overtime_trace_to_dto)
            .collect(),
    }))
}
