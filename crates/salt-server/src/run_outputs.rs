//! `GET /api/employers/{e}/payroll-runs/{r}/register` and `GET
//! /api/employers/{e}/payroll-runs/{r}/payment-summary` (issue #83, parent
//! #70 §D-9, §D-10). Both wrap `payroll_app::run_outputs`'s two read
//! models in hand-written DTOs (§0.29): the Register shows every
//! `FinalizedPayroll` a run produced, with each row's liveness and two
//! totals; the PaymentSummary shows only the Live rows, with a plain count
//! of how many were excluded as reversed.
//!
//! **This module carries facts only.** The human sentences the acceptance
//! criteria ask for — "producing this means nobody has been paid", "the
//! original may already have been paid" — belong to the screen and the PDF,
//! never to this JSON (README.md's own rule); a client derives them from
//! `liveness`/`replaces`/`excludedReversedCount`.
//!
//! Both handlers take their `EmployerId` from [`AuthorizedEmployerContext`]
//! and never from the path (ADR-0017); scoping happens in `payroll-app`'s
//! own SQL, so a run id belonging to another Employer reads back as
//! `payroll_run_not_found`, indistinguishable from an unknown one. Readable
//! by a `PayrollOperator` as well as an Owner (§0.6): neither handler calls
//! `AuthorizedEmployerContext::require_role`.

use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use chrono::{DateTime, NaiveDate, Utc};
use payroll::EmployerId;
use payroll_app::{FinalizedPayrollLiveness, PaymentSummary, PayrollRegister, RunKind};
use serde::Serialize;

use crate::authorized_employer::AuthorizedEmployerContext;
use crate::employment_facts::PayPeriodDto;
use crate::error::ApiError;
use crate::payroll_runs::{FiguresDto, figures_to_dto};
use crate::payslip::pdf_response;
use crate::run_outputs_render::{
    PaymentSummaryPdfInput, PaymentSummaryPdfRow, RegisterFiguresInput, RegisterPdfInput,
    RegisterPdfLiveness, RegisterPdfRow, render_payment_summary, render_register,
};
use crate::state::AppState;

fn run_kind_str(kind: RunKind) -> &'static str {
    match kind {
        RunKind::Ordinary => "ordinary",
        RunKind::Correction => "correction",
    }
}

/// One `FinalizedPayroll` row's liveness, tagged by `state` (issue #83):
/// `{"state": "live"}`, or `{"state": "reversed", "reason", "reversedAt",
/// "replacedBy"}`. A client branches on `state`, never on which optional
/// fields happen to be present.
#[derive(Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
enum LivenessDto {
    Live,
    #[serde(rename_all = "camelCase")]
    Reversed {
        reason: String,
        reversed_at: DateTime<Utc>,
        replaced_by: Option<String>,
    },
}

fn liveness_to_dto(liveness: FinalizedPayrollLiveness) -> LivenessDto {
    match liveness {
        FinalizedPayrollLiveness::Live => LivenessDto::Live,
        FinalizedPayrollLiveness::Reversed {
            reason,
            reversed_at,
            replaced_by,
        } => LivenessDto::Reversed {
            reason,
            reversed_at,
            replaced_by: replaced_by.map(|id| id.to_string()),
        },
    }
}

/// The same eleven figures [`FiguresDto`] carries, summed over however many
/// rows a [`PayrollRegisterResponse`] totals — reusing that DTO rather than
/// inventing a second money shape for a total (README.md: "reuse that DTO;
/// do not create a second figures shape").
fn totals_to_dto(totals: payroll_app::PayrollRegisterTotals) -> FiguresDto {
    figures_to_dto(payroll_app::PayrollFigures {
        basic_pay: totals.basic_pay,
        taxable_allowances: totals.taxable_allowances,
        overtime: totals.overtime,
        gross: totals.gross,
        taxable_remuneration: totals.taxable_remuneration,
        paye: totals.paye,
        employee_social_security: totals.employee_social_security,
        employer_social_security: totals.employer_social_security,
        medical_aid_premium: totals.medical_aid_premium,
        total_deductions: totals.total_deductions,
        net_pay: totals.net_pay,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PayrollRegisterRowDto {
    finalized_payroll_id: String,
    employment_id: String,
    full_name: String,
    figures: FiguresDto,
    liveness: LivenessDto,
    replaces: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PayrollRegisterResponse {
    payroll_run_id: String,
    kind: &'static str,
    period: PayPeriodDto,
    pay_date: NaiveDate,
    rows: Vec<PayrollRegisterRowDto>,
    total_as_finalized: FiguresDto,
    total_still_live: FiguresDto,
}

fn payroll_register_to_response(register: PayrollRegister) -> PayrollRegisterResponse {
    PayrollRegisterResponse {
        payroll_run_id: register.payroll_run_id.to_string(),
        kind: run_kind_str(register.kind),
        period: PayPeriodDto::from(register.period),
        pay_date: register.pay_date,
        rows: register
            .rows
            .into_iter()
            .map(|row| PayrollRegisterRowDto {
                finalized_payroll_id: row.finalized_payroll_id.to_string(),
                employment_id: row.employment_id.to_string(),
                full_name: row.full_name,
                figures: figures_to_dto(row.figures),
                liveness: liveness_to_dto(row.liveness),
                replaces: row.replaces.map(|id| id.to_string()),
            })
            .collect(),
        total_as_finalized: totals_to_dto(register.total_as_finalized),
        total_still_live: totals_to_dto(register.total_still_live),
    }
}

/// `GET /api/employers/{e}/payroll-runs/{r}/register`: every Employment a
/// finalized run paid, with each row's liveness and the run's two totals
/// (§D-9). 409 `payroll_run_not_finalized` for a Draft or Calculated run.
pub(crate) async fn get_register(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, payroll_run_id)): Path<(String, String)>,
) -> Result<Json<PayrollRegisterResponse>, ApiError> {
    let employer_id = EmployerId::new(context.employer_id().as_str());

    let register =
        payroll_app::get_payroll_register(state.db(), &employer_id, &payroll_run_id).await?;

    Ok(Json(payroll_register_to_response(register)))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PaymentSummaryRowDto {
    finalized_payroll_id: String,
    employment_id: String,
    full_name: String,
    net_pay_cents: i64,
    replaces: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PaymentSummaryResponse {
    payroll_run_id: String,
    kind: &'static str,
    period: PayPeriodDto,
    pay_date: NaiveDate,
    rows: Vec<PaymentSummaryRowDto>,
    excluded_reversed_count: usize,
    total_net_pay_cents: i64,
}

fn payment_summary_to_response(summary: PaymentSummary) -> PaymentSummaryResponse {
    PaymentSummaryResponse {
        payroll_run_id: summary.payroll_run_id.to_string(),
        kind: run_kind_str(summary.kind),
        period: PayPeriodDto::from(summary.period),
        pay_date: summary.pay_date,
        rows: summary
            .rows
            .into_iter()
            .map(|row| PaymentSummaryRowDto {
                finalized_payroll_id: row.finalized_payroll_id.to_string(),
                employment_id: row.employment_id.to_string(),
                full_name: row.full_name,
                net_pay_cents: row.net_pay.cents(),
                replaces: row.replaces.map(|id| id.to_string()),
            })
            .collect(),
        excluded_reversed_count: summary.excluded_reversed_count,
        total_net_pay_cents: summary.total_net_pay.cents(),
    }
}

/// `GET /api/employers/{e}/payroll-runs/{r}/payment-summary`: names and net
/// pay for a finalized run's Live records only, and how many rows were
/// excluded as reversed (§D-10). Same refusals as [`get_register`].
pub(crate) async fn get_payment_summary(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, payroll_run_id)): Path<(String, String)>,
) -> Result<Json<PaymentSummaryResponse>, ApiError> {
    let employer_id = EmployerId::new(context.employer_id().as_str());

    let summary =
        payroll_app::get_payment_summary(state.db(), &employer_id, &payroll_run_id).await?;

    Ok(Json(payment_summary_to_response(summary)))
}

fn run_kind_label(kind: RunKind) -> &'static str {
    match kind {
        RunKind::Ordinary => "Ordinary",
        RunKind::Correction => "Correction",
    }
}

fn figures_to_register_input(figures: payroll_app::PayrollFigures) -> RegisterFiguresInput {
    RegisterFiguresInput {
        basic_pay_cents: figures.basic_pay.cents(),
        taxable_allowances_cents: figures.taxable_allowances.cents(),
        overtime_cents: figures.overtime.cents(),
        gross_cents: figures.gross.cents(),
        paye_cents: figures.paye.cents(),
        employee_social_security_cents: figures.employee_social_security.cents(),
        medical_aid_premium_cents: figures.medical_aid_premium.cents(),
        total_deductions_cents: figures.total_deductions.cents(),
        net_pay_cents: figures.net_pay.cents(),
        employer_social_security_cents: figures.employer_social_security.cents(),
    }
}

/// [`payroll_app::PayrollRegisterTotals`] carries the same eleven money
/// fields [`payroll_app::PayrollFigures`] does (minus none — see
/// `totals_to_dto` above, which already leans on this) — rebuilt into a
/// `PayrollFigures` first so [`figures_to_register_input`] stays the one
/// place that maps those field names into [`RegisterFiguresInput`].
fn totals_to_register_input(totals: payroll_app::PayrollRegisterTotals) -> RegisterFiguresInput {
    figures_to_register_input(payroll_app::PayrollFigures {
        basic_pay: totals.basic_pay,
        taxable_allowances: totals.taxable_allowances,
        overtime: totals.overtime,
        gross: totals.gross,
        taxable_remuneration: totals.taxable_remuneration,
        paye: totals.paye,
        employee_social_security: totals.employee_social_security,
        employer_social_security: totals.employer_social_security,
        medical_aid_premium: totals.medical_aid_premium,
        total_deductions: totals.total_deductions,
        net_pay: totals.net_pay,
    })
}

fn liveness_to_register_input(liveness: FinalizedPayrollLiveness) -> RegisterPdfLiveness {
    match liveness {
        FinalizedPayrollLiveness::Live => RegisterPdfLiveness::Live,
        FinalizedPayrollLiveness::Reversed {
            reason,
            replaced_by,
            ..
        } => RegisterPdfLiveness::Reversed {
            reason,
            replaced_by: replaced_by.map(|id| id.to_string()),
        },
    }
}

fn register_to_render_input(register: PayrollRegister) -> RegisterPdfInput {
    RegisterPdfInput {
        payroll_run_id: register.payroll_run_id.to_string(),
        run_kind_label: run_kind_label(register.kind).to_string(),
        period_start: register.period.start(),
        period_end: register.period.end(),
        pay_date: register.pay_date,
        rows: register
            .rows
            .into_iter()
            .map(|row| RegisterPdfRow {
                full_name: row.full_name,
                figures: figures_to_register_input(row.figures),
                liveness: liveness_to_register_input(row.liveness),
                replaces: row.replaces.map(|id| id.to_string()),
            })
            .collect(),
        total_as_finalized: totals_to_register_input(register.total_as_finalized),
        total_still_live: totals_to_register_input(register.total_still_live),
    }
}

fn payment_summary_to_render_input(summary: PaymentSummary) -> PaymentSummaryPdfInput {
    PaymentSummaryPdfInput {
        payroll_run_id: summary.payroll_run_id.to_string(),
        is_correction: matches!(summary.kind, RunKind::Correction),
        period_start: summary.period.start(),
        period_end: summary.period.end(),
        pay_date: summary.pay_date,
        rows: summary
            .rows
            .into_iter()
            .map(|row| PaymentSummaryPdfRow {
                full_name: row.full_name,
                net_pay_cents: row.net_pay.cents(),
                replaces: row.replaces.map(|id| id.to_string()),
            })
            .collect(),
        excluded_reversed_count: summary.excluded_reversed_count,
        total_net_pay_cents: summary.total_net_pay.cents(),
    }
}

/// `GET /api/employers/{e}/payroll-runs/{r}/register.pdf` (issue #83): the
/// same rows and totals [`get_register`] answers as JSON, printed —
/// landscape, one row per `FinalizedPayroll`, each row's liveness, and both
/// totals. Same refusals as [`get_register`]: 409 `payroll_run_not_finalized`
/// for a Draft or Calculated run, 404 for an unknown or cross-Employer id.
pub(crate) async fn get_register_pdf(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, payroll_run_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let employer_id = EmployerId::new(context.employer_id().as_str());

    let register =
        payroll_app::get_payroll_register(state.db(), &employer_id, &payroll_run_id).await?;
    let filename = format!("register-{}.pdf", register.payroll_run_id);
    let bytes = render_register(&register_to_render_input(register));

    Ok(pdf_response(bytes, &filename))
}

/// `GET /api/employers/{e}/payroll-runs/{r}/payment-summary.pdf` (issue
/// #83): the same Live-only rows [`get_payment_summary`] answers as JSON,
/// printed — portrait, names and net pay, the excluded-as-reversed count,
/// and every acceptance-criterion sentence README.md's own Payment Summary
/// spec names. Same refusals as [`get_payment_summary`].
pub(crate) async fn get_payment_summary_pdf(
    State(state): State<AppState>,
    context: AuthorizedEmployerContext,
    Path((_employer_id, payroll_run_id)): Path<(String, String)>,
) -> Result<Response, ApiError> {
    let employer_id = EmployerId::new(context.employer_id().as_str());

    let summary =
        payroll_app::get_payment_summary(state.db(), &employer_id, &payroll_run_id).await?;
    let filename = format!("payment-summary-{}.pdf", summary.payroll_run_id);
    let bytes = render_payment_summary(&payment_summary_to_render_input(summary));

    Ok(pdf_response(bytes, &filename))
}
