//! Issue #50: the one file that decides every refusal `payroll-app` and
//! `payroll` can raise a status, a stable, permanent `code`, and its
//! structured `details` — nothing else in `salt-server` ever invents one at
//! its own call site. A client branches on `code` forever; `message` is
//! always the refusal's own `Display` and may read differently between
//! releases.
//!
//! [`classify_payroll_app_error`] and [`classify_payroll_error`] are each a
//! `match` with **no wildcard arm**: adding a variant to either enum fails
//! this crate's build until it is named here, which is the whole point —
//! nobody can ship a new refusal that reaches a caller unmapped.
//!
//! Status split (§0.24, issue #50's own Deep Instructions): **409** for
//! lifecycle, mismatch and acknowledgement conflicts; **422** for a semantic
//! payroll refusal (every [`PayrollError`] the calculator itself raises, plus
//! the handful of `PayrollAppError` variants that guard a payroll domain
//! precondition the same way); **400** for malformed input a caller gave
//! directly (a blank required reason, contradictory dates, an explicitly
//! invalid enum value); **404** for an id that names nothing, or names
//! something outside the scope the request implies; **500** only for
//! [`PayrollAppError::Database`], [`PayrollAppError::SchemaOutOfDate`] and
//! [`PayrollAppError::PasswordHashingFailed`] — failures that are never a
//! fact about what the caller asked for.
//!
//! `PayrollAppError::Payroll`, `Database` and `SchemaOutOfDate` already have
//! call-site handling elsewhere (`session.rs`'s login, in particular, maps
//! [`PayrollAppError::OperatorCredentialInvalid`] itself); this file's
//! [`classify_payroll_app_error`] agrees with every one of those existing
//! decisions; it does not have to replace them.

use axum::http::StatusCode;
use payroll::{PayrollError, UnsupportedDeductionKind};
use payroll_app::{PayrollAppError, PayrollRunBlocker, ScheduleBoundedFact};
use serde_json::{Value, json};

use crate::error::ApiError;
use crate::request_id::current_request_id;

impl From<PayrollAppError> for ApiError {
    fn from(err: PayrollAppError) -> Self {
        match classify_payroll_app_error(&err) {
            Classification::Internal => ApiError::internal(err),
            Classification::Mapped(status, code, details) => {
                ApiError::payroll_refusal(status, code, wire_message(&err), details)
            }
        }
    }
}

/// The `message` a refusal reaches a client with: the refusal's own
/// `Display`, with one exception.
///
/// The three finalization mismatches describe themselves by diffing the
/// frozen `PayrollInput`, `PayrollRules` or `PayrollCalculation` field by
/// field, so their `Display` spells out the internal field names and values
/// of a frozen snapshot. #49's own rule is that a raw frozen snapshot is
/// never a response body, precisely so its internal shape does not silently
/// become a public contract, and a `message` a client can read is a response
/// body. The diff is logged instead — it is what an engineer wants and no
/// client should depend on — and the caller gets §0.26's own recovery
/// sentence, with the Employment named in `details` (§0.26 again: naming the
/// employment that moved is what the refusal owes).
fn wire_message(err: &PayrollAppError) -> String {
    let describes_a_frozen_snapshot = matches!(
        err,
        PayrollAppError::FinalizationInputMismatch { .. }
            | PayrollAppError::FinalizationRulesMismatch { .. }
            | PayrollAppError::FinalizationCalculationMismatch { .. }
    );
    if describes_a_frozen_snapshot {
        tracing::warn!(
            error = %err,
            request_id = %current_request_id(),
            "finalization refused: the facts moved since the run was calculated"
        );
        return "the facts changed since this run was calculated; calculate it again".to_string();
    }
    err.to_string()
}

/// The outcome of classifying one refusal: either it belongs on the
/// existing internal-error path ([`ApiError::internal`], which logs the
/// cause in full and echoes only `details.requestId`), or it is mapped to a
/// documented status, `code` and `details` of its own.
enum Classification {
    Internal,
    Mapped(StatusCode, &'static str, Option<Value>),
}

/// Every [`PayrollAppError`] variant, mapped once. See the module doc for
/// the status split and the no-wildcard discipline this `match` keeps.
fn classify_payroll_app_error(err: &PayrollAppError) -> Classification {
    match err {
        PayrollAppError::Payroll(inner) => {
            let (status, code, details) = classify_payroll_error(inner);
            Classification::Mapped(status, code, details)
        }
        // The unexpected: logged in full by `ApiError::internal`, never
        // echoed into a response body (issue #50's own acceptance
        // criterion — no mapped body carries SQL text or a stack trace).
        PayrollAppError::Database(_)
        | PayrollAppError::SchemaOutOfDate { .. }
        | PayrollAppError::PasswordHashingFailed(_) => Classification::Internal,

        PayrollAppError::EmployerNameCannotBeEmpty => Classification::Mapped(
            StatusCode::BAD_REQUEST,
            "employer_name_cannot_be_empty",
            None,
        ),
        PayrollAppError::EmployerNotFound(id) => Classification::Mapped(
            StatusCode::NOT_FOUND,
            "employer_not_found",
            Some(json!({ "employerId": id.to_string() })),
        ),
        PayrollAppError::EmploymentNotFound(id) => Classification::Mapped(
            StatusCode::NOT_FOUND,
            "employment_not_found",
            Some(json!({ "employmentId": id.to_string() })),
        ),
        PayrollAppError::PersonNotFound(id) => Classification::Mapped(
            StatusCode::NOT_FOUND,
            "person_not_found",
            Some(json!({ "personId": id.to_string() })),
        ),
        PayrollAppError::PersonFullNameCannotBeEmpty => Classification::Mapped(
            StatusCode::BAD_REQUEST,
            "person_full_name_cannot_be_empty",
            None,
        ),
        PayrollAppError::EmploymentIsVoid(id) => Classification::Mapped(
            StatusCode::CONFLICT,
            "employment_is_void",
            Some(json!({ "employmentId": id.to_string() })),
        ),
        PayrollAppError::EmploymentEndsBeforeItStarts {
            start_date,
            end_date,
        } => Classification::Mapped(
            StatusCode::BAD_REQUEST,
            "employment_ends_before_it_starts",
            Some(json!({
                "startDate": start_date.to_string(),
                "endDate": end_date.to_string(),
            })),
        ),
        PayrollAppError::NoCompensationTermsInForce(id) => Classification::Mapped(
            StatusCode::UNPROCESSABLE_ENTITY,
            "no_compensation_terms_in_force",
            Some(json!({ "employmentId": id.to_string() })),
        ),
        PayrollAppError::PriorEmploymentDeclarationCannotBeUnknown => Classification::Mapped(
            StatusCode::BAD_REQUEST,
            "prior_employment_declaration_cannot_be_unknown",
            None,
        ),
        PayrollAppError::UnsupportedDeductionDeclarationCannotBeUnknown => Classification::Mapped(
            StatusCode::BAD_REQUEST,
            "unsupported_deduction_declaration_cannot_be_unknown",
            None,
        ),
        PayrollAppError::SaltCoverageStartNotAPeriodEnd {
            salt_coverage_start,
        } => Classification::Mapped(
            StatusCode::UNPROCESSABLE_ENTITY,
            "salt_coverage_start_not_a_period_end",
            Some(json!({ "saltCoverageStart": salt_coverage_start.to_string() })),
        ),
        PayrollAppError::SaltCoverageStartOutsideTaxYear {
            salt_coverage_start,
            tax_year,
        } => Classification::Mapped(
            StatusCode::UNPROCESSABLE_ENTITY,
            "salt_coverage_start_outside_tax_year",
            Some(json!({
                "saltCoverageStart": salt_coverage_start.to_string(),
                "taxYear": tax_year.starting_year(),
            })),
        ),
        PayrollAppError::SaltCoverageStartBeforeEmploymentIsPayable {
            salt_coverage_start,
            first_payable_period_end,
        } => Classification::Mapped(
            StatusCode::UNPROCESSABLE_ENTITY,
            "salt_coverage_start_before_employment_is_payable",
            Some(json!({
                "saltCoverageStart": salt_coverage_start.to_string(),
                "firstPayablePeriodEnd": first_payable_period_end.to_string(),
            })),
        ),
        PayrollAppError::OpeningBalanceFiguresOverAnEmptyCoveredSpan {
            salt_coverage_start,
        } => Classification::Mapped(
            StatusCode::UNPROCESSABLE_ENTITY,
            "opening_balance_figures_over_an_empty_covered_span",
            Some(json!({ "saltCoverageStart": salt_coverage_start.to_string() })),
        ),
        PayrollAppError::PayPeriodNotGeneratedByThePaySchedule {
            period,
            schedules_period,
        } => Classification::Mapped(
            StatusCode::UNPROCESSABLE_ENTITY,
            "pay_period_not_generated_by_the_pay_schedule",
            Some(json!({
                "periodStart": period.start().to_string(),
                "periodEnd": period.end().to_string(),
                "schedulesPeriodStart": schedules_period.start().to_string(),
                "schedulesPeriodEnd": schedules_period.end().to_string(),
            })),
        ),
        PayrollAppError::PayrollRunNotFound(id) => Classification::Mapped(
            StatusCode::NOT_FOUND,
            "payroll_run_not_found",
            Some(json!({ "payrollRunId": id.to_string() })),
        ),
        PayrollAppError::PayrollRunIsNotOrdinary(id) => Classification::Mapped(
            StatusCode::CONFLICT,
            "payroll_run_is_not_ordinary",
            Some(json!({ "payrollRunId": id.to_string() })),
        ),
        PayrollAppError::RemovalReasonCannotBeEmpty => Classification::Mapped(
            StatusCode::BAD_REQUEST,
            "removal_reason_cannot_be_empty",
            None,
        ),
        PayrollAppError::EmploymentNotAnActiveRunMember {
            payroll_run_id,
            employment_id,
        } => Classification::Mapped(
            StatusCode::CONFLICT,
            "employment_not_an_active_run_member",
            Some(json!({
                "payrollRunId": payroll_run_id.to_string(),
                "employmentId": employment_id.to_string(),
            })),
        ),
        PayrollAppError::BasicPayCannotBeSetAsAnEarning => Classification::Mapped(
            StatusCode::UNPROCESSABLE_ENTITY,
            "basic_pay_cannot_be_set_as_an_earning",
            None,
        ),
        // §0.28: the client that lost a Finalize response reads this code
        // and shows the success that already happened, so `details` names
        // every `FinalizedPayroll` the run produced. `finalizedPayrollId`
        // is the direct-navigation shortcut for the case with one answer —
        // a Correction run always, an Ordinary run of one member — and is
        // `null` when the run has more than one, where `finalizedPayrolls`
        // is the only truthful answer.
        PayrollAppError::PayrollRunAlreadyFinalized {
            payroll_run_id,
            finalized_payrolls,
        } => Classification::Mapped(
            StatusCode::CONFLICT,
            "payroll_run_already_finalized",
            Some(json!({
                "payrollRunId": payroll_run_id.to_string(),
                "finalizedPayrollId": if let [(_, only)] = finalized_payrolls.as_slice() {
                    Some(only.to_string())
                } else {
                    None
                },
                "finalizedPayrolls": finalized_payrolls
                    .iter()
                    .map(|(employment_id, finalized_payroll_id)| json!({
                        "employmentId": employment_id.to_string(),
                        "finalizedPayrollId": finalized_payroll_id.to_string(),
                    }))
                    .collect::<Vec<_>>(),
            })),
        ),
        PayrollAppError::PayrollRunNotCalculated(id) => Classification::Mapped(
            StatusCode::CONFLICT,
            "payroll_run_not_calculated",
            Some(json!({ "payrollRunId": id.to_string() })),
        ),
        // The nested refusal's own `Display` is never repeated into this
        // response: it is already embedded in this variant's own `Display`
        // (used as this refusal's `message` once we return `Mapped`), and
        // when the nested refusal is itself `Internal` — a `Database` read
        // that failed while rebuilding this member — the whole thing must
        // become `Internal` too, so that cause is logged, never echoed, the
        // same as at the top level.
        PayrollAppError::FinalizationRebuildRefused {
            employment_id,
            refusal,
        } => match classify_payroll_app_error(refusal) {
            Classification::Internal => Classification::Internal,
            // The nested refusal's own `details` are carried through
            // unchanged: "finalizing Alice refused" without the kinds of
            // unsupported deduction, or the period nothing covers, is a
            // refusal nobody can act on.
            Classification::Mapped(_, inner_code, inner_details) => Classification::Mapped(
                StatusCode::CONFLICT,
                "finalization_rebuild_refused",
                Some(json!({
                    "employmentId": employment_id.to_string(),
                    "refusalCode": inner_code,
                    "refusalDetails": inner_details,
                })),
            ),
        },
        PayrollAppError::FinalizationInputMismatch { employment_id, .. } => Classification::Mapped(
            StatusCode::CONFLICT,
            "finalization_input_mismatch",
            Some(json!({ "employmentId": employment_id.to_string() })),
        ),
        PayrollAppError::FinalizationRulesMismatch { employment_id, .. } => Classification::Mapped(
            StatusCode::CONFLICT,
            "finalization_rules_mismatch",
            Some(json!({ "employmentId": employment_id.to_string() })),
        ),
        PayrollAppError::FinalizationCalculationMismatch { employment_id, .. } => {
            Classification::Mapped(
                StatusCode::CONFLICT,
                "finalization_calculation_mismatch",
                Some(json!({ "employmentId": employment_id.to_string() })),
            )
        }
        PayrollAppError::FinalizedPayrollNotFound(id) => Classification::Mapped(
            StatusCode::NOT_FOUND,
            "finalized_payroll_not_found",
            Some(json!({ "finalizedPayrollId": id.to_string() })),
        ),
        PayrollAppError::FinalizedPayrollAlreadyReversed(id) => Classification::Mapped(
            StatusCode::CONFLICT,
            "finalized_payroll_already_reversed",
            Some(json!({ "finalizedPayrollId": id.to_string() })),
        ),
        PayrollAppError::ReversalReasonCannotBeEmpty => Classification::Mapped(
            StatusCode::BAD_REQUEST,
            "reversal_reason_cannot_be_empty",
            None,
        ),
        PayrollAppError::OpeningBalanceFrozenByFinalization {
            employment_id,
            tax_year,
        } => Classification::Mapped(
            StatusCode::CONFLICT,
            "opening_balance_frozen_by_finalization",
            Some(json!({
                "employmentId": employment_id.to_string(),
                "taxYear": tax_year.starting_year(),
            })),
        ),
        PayrollAppError::PriorEmploymentFrozenByFinalization {
            employment_id,
            tax_year,
        } => Classification::Mapped(
            StatusCode::CONFLICT,
            "prior_employment_frozen_by_finalization",
            Some(json!({
                "employmentId": employment_id.to_string(),
                "taxYear": tax_year.starting_year(),
            })),
        ),
        PayrollAppError::PayScheduleFrozenByFinalization {
            employer_id,
            tax_year,
        } => Classification::Mapped(
            StatusCode::CONFLICT,
            "pay_schedule_frozen_by_finalization",
            Some(json!({
                "employerId": employer_id.to_string(),
                "taxYear": tax_year.starting_year(),
            })),
        ),
        PayrollAppError::PayScheduleChangeBlockedByAnOpenRun {
            employer_id,
            period_end,
        } => Classification::Mapped(
            StatusCode::CONFLICT,
            "pay_schedule_change_blocked_by_an_open_run",
            Some(json!({
                "employerId": employer_id.to_string(),
                "periodEnd": period_end.to_string(),
            })),
        ),
        PayrollAppError::PayScheduleMovedWithinTaxYear {
            employer_id,
            tax_year,
            finalized_period_end,
        } => Classification::Mapped(
            StatusCode::CONFLICT,
            "pay_schedule_moved_within_tax_year",
            Some(json!({
                "employerId": employer_id.to_string(),
                "taxYear": tax_year.starting_year(),
                "finalizedPeriodEnd": finalized_period_end.to_string(),
            })),
        ),
        PayrollAppError::PayScheduleChangeWouldStrandAStoredBoundary {
            employer_id,
            fact,
            boundary,
        } => Classification::Mapped(
            StatusCode::CONFLICT,
            "pay_schedule_change_would_strand_a_stored_boundary",
            Some(json!({
                "employerId": employer_id.to_string(),
                "fact": schedule_bounded_fact_code(fact),
                "boundary": boundary.to_string(),
            })),
        ),
        PayrollAppError::TaxYearOutsideRepresentableCalendar { tax_year } => {
            Classification::Mapped(
                StatusCode::UNPROCESSABLE_ENTITY,
                "tax_year_outside_representable_calendar",
                Some(json!({ "taxYear": tax_year.starting_year() })),
            )
        }
        PayrollAppError::PrecedingPeriodUnresolved {
            employment_id,
            period,
        } => Classification::Mapped(
            StatusCode::CONFLICT,
            "preceding_period_unresolved",
            Some(json!({
                "employmentId": employment_id.to_string(),
                "periodStart": period.start().to_string(),
                "periodEnd": period.end().to_string(),
            })),
        ),
        PayrollAppError::CorrectionReasonCannotBeEmpty => Classification::Mapped(
            StatusCode::BAD_REQUEST,
            "correction_reason_cannot_be_empty",
            None,
        ),
        PayrollAppError::PayrollRunIsNotCorrection(id) => Classification::Mapped(
            StatusCode::CONFLICT,
            "payroll_run_is_not_correction",
            Some(json!({ "payrollRunId": id.to_string() })),
        ),
        PayrollAppError::CorrectionRunAlreadyHasAnEmployment(id) => Classification::Mapped(
            StatusCode::CONFLICT,
            "correction_run_already_has_an_employment",
            Some(json!({ "payrollRunId": id.to_string() })),
        ),
        PayrollAppError::CorrectionEmploymentBelongsToADifferentEmployer {
            employment_id,
            employer_id,
        } => Classification::Mapped(
            StatusCode::NOT_FOUND,
            "correction_employment_belongs_to_a_different_employer",
            Some(json!({
                "employmentId": employment_id.to_string(),
                "employerId": employer_id.to_string(),
            })),
        ),
        PayrollAppError::CorrectionTargetDoesNotMatch {
            payroll_run_id,
            finalized_payroll_id,
        } => Classification::Mapped(
            StatusCode::CONFLICT,
            "correction_target_does_not_match",
            Some(json!({
                "payrollRunId": payroll_run_id.to_string(),
                "finalizedPayrollId": finalized_payroll_id.to_string(),
            })),
        ),
        PayrollAppError::CorrectionTargetNotReversed(id) => Classification::Mapped(
            StatusCode::CONFLICT,
            "correction_target_not_reversed",
            Some(json!({ "finalizedPayrollId": id.to_string() })),
        ),
        PayrollAppError::CorrectionTargetAlreadyReplaced(id) => Classification::Mapped(
            StatusCode::CONFLICT,
            "correction_target_already_replaced",
            Some(json!({ "finalizedPayrollId": id.to_string() })),
        ),
        PayrollAppError::CorrectionLineageNotLegitimate {
            employment_id,
            period,
        } => Classification::Mapped(
            StatusCode::CONFLICT,
            "correction_lineage_not_legitimate",
            Some(json!({
                "employmentId": employment_id.to_string(),
                "periodStart": period.start().to_string(),
                "periodEnd": period.end().to_string(),
            })),
        ),
        PayrollAppError::CorrectionLineageOmitsAReversedPredecessor {
            employment_id,
            period,
            finalized_payroll_id,
        } => Classification::Mapped(
            StatusCode::CONFLICT,
            "correction_lineage_omits_a_reversed_predecessor",
            Some(json!({
                "employmentId": employment_id.to_string(),
                "periodStart": period.start().to_string(),
                "periodEnd": period.end().to_string(),
                "finalizedPayrollId": finalized_payroll_id.to_string(),
            })),
        ),
        PayrollAppError::CorrectionPeriodAlreadyHasALivePayroll {
            employment_id,
            period,
        } => Classification::Mapped(
            StatusCode::CONFLICT,
            "correction_period_already_has_a_live_payroll",
            Some(json!({
                "employmentId": employment_id.to_string(),
                "periodStart": period.start().to_string(),
                "periodEnd": period.end().to_string(),
            })),
        ),
        PayrollAppError::CompensationTermsCorrectionReasonCannotBeEmpty => Classification::Mapped(
            StatusCode::BAD_REQUEST,
            "compensation_terms_correction_reason_cannot_be_empty",
            None,
        ),
        PayrollAppError::NoCompensationTermsRowAt {
            employment_id,
            effective_from,
        } => Classification::Mapped(
            StatusCode::NOT_FOUND,
            "no_compensation_terms_row_at",
            Some(json!({
                "employmentId": employment_id.to_string(),
                "effectiveFrom": effective_from.to_string(),
            })),
        ),
        PayrollAppError::UnsupportedDeductionDeclarationReasonCannotBeEmpty => {
            Classification::Mapped(
                StatusCode::BAD_REQUEST,
                "unsupported_deduction_declaration_reason_cannot_be_empty",
                None,
            )
        }
        PayrollAppError::MasterDataDivergenceNotAcknowledged {
            employment_id,
            diverging_periods,
        } => Classification::Mapped(
            StatusCode::CONFLICT,
            "master_data_divergence_not_acknowledged",
            Some(json!({
                "employmentId": employment_id.to_string(),
                "divergingPeriods": diverging_periods
                    .iter()
                    .map(|period| json!({
                        "start": period.start().to_string(),
                        "end": period.end().to_string(),
                    }))
                    .collect::<Vec<_>>(),
            })),
        ),
        PayrollAppError::CompensationTermsAlreadyExistAt {
            employment_id,
            effective_from,
        } => Classification::Mapped(
            StatusCode::CONFLICT,
            "compensation_terms_already_exist_at",
            Some(json!({
                "employmentId": employment_id.to_string(),
                "effectiveFrom": effective_from.to_string(),
            })),
        ),
        PayrollAppError::OperatorEmailCannotBeEmpty => Classification::Mapped(
            StatusCode::BAD_REQUEST,
            "operator_email_cannot_be_empty",
            None,
        ),
        PayrollAppError::OperatorDisplayNameCannotBeEmpty => Classification::Mapped(
            StatusCode::BAD_REQUEST,
            "operator_display_name_cannot_be_empty",
            None,
        ),
        PayrollAppError::OperatorPasswordTooShort { minimum } => Classification::Mapped(
            StatusCode::BAD_REQUEST,
            "operator_password_too_short",
            Some(json!({ "minimum": minimum })),
        ),
        PayrollAppError::OperatorPasswordTooLong { maximum } => Classification::Mapped(
            StatusCode::BAD_REQUEST,
            "operator_password_too_long",
            Some(json!({ "maximum": maximum })),
        ),
        PayrollAppError::OperatorEmailAlreadyInUse => {
            Classification::Mapped(StatusCode::CONFLICT, "operator_email_already_in_use", None)
        }
        PayrollAppError::OperatorNotFound(id) => Classification::Mapped(
            StatusCode::NOT_FOUND,
            "operator_not_found",
            Some(json!({ "operatorId": id.to_string() })),
        ),
        PayrollAppError::OperatorAlreadyDisabled(id) => Classification::Mapped(
            StatusCode::CONFLICT,
            "operator_already_disabled",
            Some(json!({ "operatorId": id.to_string() })),
        ),
        // Agrees with `session.rs`'s own `ApiError::invalid_credentials()`
        // (issue #46) rather than replacing it: every kind of login failure
        // must look identical from outside, and that refusal already is.
        PayrollAppError::OperatorCredentialInvalid => {
            Classification::Mapped(StatusCode::UNAUTHORIZED, "invalid_credentials", None)
        }
        PayrollAppError::EmployerMembershipAlreadyExists {
            operator_id,
            employer_id,
        } => Classification::Mapped(
            StatusCode::CONFLICT,
            "employer_membership_already_exists",
            Some(json!({
                "operatorId": operator_id.to_string(),
                "employerId": employer_id.to_string(),
            })),
        ),
        PayrollAppError::EmployerMembershipNotFound {
            operator_id,
            employer_id,
        } => Classification::Mapped(
            StatusCode::NOT_FOUND,
            "employer_membership_not_found",
            Some(json!({
                "operatorId": operator_id.to_string(),
                "employerId": employer_id.to_string(),
            })),
        ),
        PayrollAppError::EmployerMembershipAlreadyRevoked {
            operator_id,
            employer_id,
        } => Classification::Mapped(
            StatusCode::CONFLICT,
            "employer_membership_already_revoked",
            Some(json!({
                "operatorId": operator_id.to_string(),
                "employerId": employer_id.to_string(),
            })),
        ),
        PayrollAppError::BootstrapOperatorAlreadyExists => Classification::Mapped(
            StatusCode::CONFLICT,
            "bootstrap_operator_already_exists",
            None,
        ),
        PayrollAppError::BootstrapOperatorTableBusy => {
            Classification::Mapped(StatusCode::CONFLICT, "bootstrap_operator_table_busy", None)
        }
        PayrollAppError::BootstrapPeriodEndDayInvalid { day } => Classification::Mapped(
            StatusCode::BAD_REQUEST,
            "bootstrap_period_end_day_invalid",
            Some(json!({ "day": day })),
        ),
    }
}

/// Every [`PayrollError`] variant, mapped once. All 422: each one is
/// `calculate`'s own semantic refusal about a payroll input, never
/// malformed shape and never an unexpected failure (see the module doc's
/// status split).
fn classify_payroll_error(err: &PayrollError) -> (StatusCode, &'static str, Option<Value>) {
    let unprocessable = StatusCode::UNPROCESSABLE_ENTITY;
    match err {
        PayrollError::CompensationTermsDoNotCoverPeriod => (
            unprocessable,
            "compensation_terms_do_not_cover_period",
            None,
        ),
        PayrollError::PayPeriodNotOnTheEmployersSchedule { expected } => (
            unprocessable,
            "pay_period_not_on_the_employers_schedule",
            Some(json!({
                "expectedPeriodStart": expected.start().to_string(),
                "expectedPeriodEnd": expected.end().to_string(),
            })),
        ),
        PayrollError::PayScheduleOutsideRepresentableCalendar { date } => (
            unprocessable,
            "pay_schedule_outside_representable_calendar",
            Some(json!({ "date": date.to_string() })),
        ),
        PayrollError::EffectiveFromNotAPeriodStart {
            next_valid_effective_from,
        } => (
            unprocessable,
            "effective_from_not_a_period_start",
            Some(json!({ "nextValidEffectiveFrom": next_valid_effective_from.to_string() })),
        ),
        PayrollError::ContradictoryEmploymentDates => {
            (unprocessable, "contradictory_employment_dates", None)
        }
        PayrollError::EmploymentDoesNotOverlapPeriod => {
            (unprocessable, "employment_does_not_overlap_period", None)
        }
        PayrollError::DuplicateBasicPayLine => (unprocessable, "duplicate_basic_pay_line", None),
        PayrollError::PriorPayeExceedsRecalculatedLiability => (
            unprocessable,
            "prior_paye_exceeds_recalculated_liability",
            None,
        ),
        PayrollError::DeductionsExceedGrossRemuneration => {
            (unprocessable, "deductions_exceed_gross_remuneration", None)
        }
        PayrollError::AmountOverflow => (unprocessable, "amount_overflow", None),
        PayrollError::NoPayeTableCoversDate { date } => (
            unprocessable,
            "no_paye_table_covers_date",
            Some(json!({ "date": date.to_string() })),
        ),
        PayrollError::OverlappingPayeTables { first, second } => (
            unprocessable,
            "overlapping_paye_tables",
            Some(json!({ "first": first.to_string(), "second": second.to_string() })),
        ),
        PayrollError::NoSscRulesetCoversDate { date } => (
            unprocessable,
            "no_ssc_ruleset_covers_date",
            Some(json!({ "date": date.to_string() })),
        ),
        PayrollError::OverlappingSscRulesets { first, second } => (
            unprocessable,
            "overlapping_ssc_rulesets",
            Some(json!({ "first": first.to_string(), "second": second.to_string() })),
        ),
        PayrollError::PayeTableDoesNotCoverPeriod { table, period_end } => (
            unprocessable,
            "paye_table_does_not_cover_period",
            Some(json!({ "table": table.to_string(), "periodEnd": period_end.to_string() })),
        ),
        PayrollError::SscRulesetDoesNotCoverPeriod {
            ruleset,
            period_end,
        } => (
            unprocessable,
            "ssc_ruleset_does_not_cover_period",
            Some(json!({ "ruleset": ruleset.to_string(), "periodEnd": period_end.to_string() })),
        ),
        PayrollError::WrongTaxYearForPeriod { expected, supplied } => (
            unprocessable,
            "wrong_tax_year_for_period",
            Some(json!({
                "expectedTaxYear": expected.starting_year(),
                "suppliedTaxYear": supplied.starting_year(),
            })),
        ),
        PayrollError::UnsupportedDeductionStatusUnknown => {
            (unprocessable, "unsupported_deduction_status_unknown", None)
        }
        PayrollError::UnsupportedDeductionsPresent { kinds } => (
            unprocessable,
            "unsupported_deductions_present",
            Some(json!({
                "kinds": kinds
                    .as_slice()
                    .iter()
                    .map(unsupported_deduction_kind_code)
                    .collect::<Vec<_>>(),
            })),
        ),
        PayrollError::PriorEmploymentUnknown => (unprocessable, "prior_employment_unknown", None),
        // Named for the refusal's own meaning, not for the Rust variant
        // (issue #50's Deep Instructions): the figures being *present* is
        // not the refusal — how a new Employer must treat them is
        // unconfirmed. §0.31 gives the run detail's `blockers` this exact
        // code for the same standing fact, and the two must not drift.
        PayrollError::PriorEmploymentPresent { figures } => (
            unprocessable,
            "prior_employment_treatment_unconfirmed",
            Some(json!({
                "taxableRemunerationCents": figures.taxable_remuneration().cents(),
                "payeCents": figures.paye().cents(),
            })),
        ),
    }
}

/// Every [`PayrollRunBlocker`] variant, mapped to the run detail's own
/// `blockers[].code` and `blockers[].details` (issue #54, §0.31). The five
/// codes below are exactly the strings [`classify_payroll_error`] already
/// uses for `unsupported_deduction_status_unknown`,
/// `unsupported_deductions_present`, `prior_employment_unknown` and
/// `prior_employment_treatment_unconfirmed`, and the one
/// [`classify_payroll_app_error`] uses for `no_compensation_terms_in_force`
/// — this function does not mint parallel ones, and its two "present"
/// bodies (`taxableRemunerationCents`/`payeCents`, `kinds`) are shaped
/// identically to those refusals' own `details` for the same reason.
pub(crate) fn blocker_code_and_details(
    blocker: &PayrollRunBlocker,
) -> (&'static str, Option<Value>) {
    match blocker {
        PayrollRunBlocker::PriorEmploymentUnknown => ("prior_employment_unknown", None),
        PayrollRunBlocker::PriorEmploymentTreatmentUnconfirmed { figures } => (
            "prior_employment_treatment_unconfirmed",
            Some(json!({
                "taxableRemunerationCents": figures.taxable_remuneration().cents(),
                "payeCents": figures.paye().cents(),
            })),
        ),
        PayrollRunBlocker::UnsupportedDeductionStatusUnknown => {
            ("unsupported_deduction_status_unknown", None)
        }
        PayrollRunBlocker::UnsupportedDeductionsPresent { kinds } => (
            "unsupported_deductions_present",
            Some(json!({
                "kinds": kinds
                    .as_slice()
                    .iter()
                    .map(unsupported_deduction_kind_code)
                    .collect::<Vec<_>>(),
            })),
        ),
        PayrollRunBlocker::NoCompensationTermsInForce => ("no_compensation_terms_in_force", None),
    }
}

/// Permanent, hand-picked `snake_case` names — never the `#[derive]`d
/// serialization of the domain enum, which is free to change shape for
/// storage reasons no API contract should feel. The HTTP request parser uses
/// this same table, so the codes a client submits and the codes a refusal
/// reports cannot drift.
const UNSUPPORTED_DEDUCTION_KIND_CODES: &[(UnsupportedDeductionKind, &str)] = &[
    (
        UnsupportedDeductionKind::ApprovedPensionFund,
        "approved_pension_fund",
    ),
    (UnsupportedDeductionKind::ProvidentFund, "provident_fund"),
    (
        UnsupportedDeductionKind::RetirementAnnuityFund,
        "retirement_annuity_fund",
    ),
    (
        UnsupportedDeductionKind::EducationPolicy,
        "education_policy",
    ),
];

pub(crate) fn unsupported_deduction_kind_code(kind: &UnsupportedDeductionKind) -> &'static str {
    UNSUPPORTED_DEDUCTION_KIND_CODES
        .iter()
        .find_map(|(candidate, code)| (candidate == kind).then_some(*code))
        .expect("every UnsupportedDeductionKind has a stable wire code")
}

pub(crate) fn parse_unsupported_deduction_kind(code: &str) -> Option<UnsupportedDeductionKind> {
    UNSUPPORTED_DEDUCTION_KIND_CODES
        .iter()
        .find_map(|(kind, candidate)| (*candidate == code).then_some(*kind))
}

/// Same reason as [`unsupported_deduction_kind_code`]: a stable wire name
/// independent of the Rust variant's own spelling.
fn schedule_bounded_fact_code(fact: &ScheduleBoundedFact) -> &'static str {
    match fact {
        ScheduleBoundedFact::SaltCoverageStart => "salt_coverage_start",
        ScheduleBoundedFact::CompensationTermsEffectiveFrom => "compensation_terms_effective_from",
        ScheduleBoundedFact::UnsupportedDeductionEffectiveFrom => {
            "unsupported_deduction_effective_from"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::response::IntoResponse;
    use chrono::NaiveDate;
    use payroll::{
        CompensationTerms, Earning, EmployerId, EmploymentId, EmploymentSnapshot, Money, PayPeriod,
        PaySchedule, PayeTableId, PayrollCalculation, PayrollInput, PayrollRules, PeriodEndDay,
        PersonId, PersonReference, PriorEmployment, PriorEmploymentFigures, SscRulesId, TaxYear,
        UnsupportedDeductionKinds, UnsupportedDeductionStatus, YearToDateContext, calculate,
        ruleset_for,
    };
    use payroll_app::{DatabaseConfig, FinalizedPayrollId, OperatorId, PayrollRunId, SaltDatabase};

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn period() -> PayPeriod {
        PayPeriod::new(date(2026, 3, 1), date(2026, 3, 31)).unwrap()
    }

    /// A real, calculable `PayrollInput`/`PayrollRules`/`PayrollCalculation`
    /// triple — needed only because the three finalization-mismatch variants
    /// carry these types, never because the mapping reads them: `details`
    /// for all three names only the `employment_id` (§0.26). Built from
    /// `payroll`'s own public constructors and `ruleset_for`/`calculate`,
    /// entirely in-process — no database involved.
    /// The same `PayrollInput` as [`a_calculable_input_rules_and_calculation`]
    /// but with its own earning lines, so a test can hold two inputs that
    /// genuinely differ. `PayrollInput`'s fields are private, which is the
    /// point: only its constructor builds one.
    fn an_input_with_earnings(earnings: Vec<Earning>) -> PayrollInput {
        let period = period();
        let compensation_terms =
            CompensationTerms::new(period.start(), None, Money::from_cents(1_500_000).unwrap())
                .unwrap();
        let snapshot = EmploymentSnapshot::new(
            EmploymentId::new("employment-1"),
            EmployerId::new("employer-1"),
            PersonReference::new(PersonId::new("person-1")),
            period.start(),
            None,
            compensation_terms,
        );
        PayrollInput::new(
            snapshot,
            period,
            earnings,
            YearToDateContext::first_period_with_no_prior_employment(TaxYear::for_period_end(
                period.end(),
            )),
            PaySchedule::new(PeriodEndDay::LastDayOfMonth),
            UnsupportedDeductionStatus::ConfirmedNone,
        )
    }

    fn a_calculable_input_rules_and_calculation() -> (PayrollInput, PayrollRules, PayrollCalculation)
    {
        let input = an_input_with_earnings(Vec::new());
        let rules = ruleset_for(period().end()).unwrap();
        let calculation = calculate(&input, &rules).unwrap();
        (input, rules, calculation)
    }

    fn check(err: PayrollAppError, status: StatusCode, code: &str, details: Option<Value>) {
        let api_err: ApiError = err.into();
        assert_eq!(api_err.status(), status, "status for {code}");
        assert_eq!(api_err.code(), code, "code");
        assert_eq!(api_err.details(), details.as_ref(), "details for {code}");
    }

    fn check_internal(err: PayrollAppError) {
        let api_err: ApiError = err.into();
        assert_eq!(api_err.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(api_err.code(), "internal_error");
        let details = api_err
            .details()
            .expect("a 500 carries details.requestId")
            .as_object()
            .expect("details is an object");
        assert!(details.contains_key("requestId"), "{details:?}");
        assert_eq!(
            details.len(),
            1,
            "a 500 carries requestId and nothing else, got {details:?}"
        );
    }

    fn check_payroll_error(err: PayrollError, code: &str, details: Option<Value>) {
        let (status, actual_code, actual_details) = classify_payroll_error(&err);
        assert_eq!(
            status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "status for {code}"
        );
        assert_eq!(actual_code, code, "code");
        assert_eq!(actual_details, details, "details for {code}");
    }

    // ---- the unexpected: 500, logged in full, echoed as nothing but a request id ----

    #[test]
    fn database_is_internal() {
        check_internal(PayrollAppError::Database("connection refused".to_string()));
    }

    #[test]
    fn schema_out_of_date_is_internal() {
        check_internal(PayrollAppError::SchemaOutOfDate {
            compiled: 10,
            applied: Some(9),
        });
    }

    #[test]
    fn password_hashing_failed_is_internal() {
        check_internal(PayrollAppError::PasswordHashingFailed("boom".to_string()));
    }

    #[tokio::test]
    async fn an_internal_causes_sql_text_never_reaches_the_response_body() {
        let cause = "relation \"employer\" does not exist at line 1: SELECT * FROM employer";
        let api_err: ApiError = PayrollAppError::Database(cause.to_string()).into();

        let response = api_err.into_response();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read the response body");
        let body = String::from_utf8(bytes.to_vec()).expect("the body is UTF-8 JSON");

        assert!(
            !body.contains("employer") && !body.contains("SELECT"),
            "no mapped body may contain SQL text (issue #50), got {body}"
        );
    }

    // ---- delegation to the payroll calculator's own mapping ----

    #[test]
    fn the_payroll_variant_delegates_to_the_payroll_error_mapping() {
        check(
            PayrollAppError::Payroll(PayrollError::AmountOverflow),
            StatusCode::UNPROCESSABLE_ENTITY,
            "amount_overflow",
            None,
        );
    }

    // ---- FinalizationRebuildRefused: recursion into the nested refusal ----

    #[test]
    fn finalization_rebuild_refused_names_the_employment_and_the_inner_code() {
        let employment_id = EmploymentId::new("employment-1");
        check(
            PayrollAppError::FinalizationRebuildRefused {
                employment_id: employment_id.clone(),
                refusal: Box::new(PayrollAppError::EmploymentIsVoid(employment_id.clone())),
            },
            StatusCode::CONFLICT,
            "finalization_rebuild_refused",
            Some(json!({
                "employmentId": employment_id.to_string(),
                "refusalCode": "employment_is_void",
                "refusalDetails": { "employmentId": employment_id.to_string() },
            })),
        );
    }

    /// A nested refusal with no `details` of its own still says so
    /// explicitly, rather than leaving the key out of the envelope.
    #[test]
    fn finalization_rebuild_refused_carries_a_null_when_the_inner_refusal_has_no_details() {
        let employment_id = EmploymentId::new("employment-1");
        check(
            PayrollAppError::FinalizationRebuildRefused {
                employment_id: employment_id.clone(),
                refusal: Box::new(PayrollAppError::Payroll(
                    PayrollError::PriorEmploymentUnknown,
                )),
            },
            StatusCode::CONFLICT,
            "finalization_rebuild_refused",
            Some(json!({
                "employmentId": employment_id.to_string(),
                "refusalCode": "prior_employment_unknown",
                "refusalDetails": null,
            })),
        );
    }

    #[test]
    fn finalization_rebuild_refused_is_itself_internal_when_the_inner_refusal_is() {
        let employment_id = EmploymentId::new("employment-1");
        check_internal(PayrollAppError::FinalizationRebuildRefused {
            employment_id,
            refusal: Box::new(PayrollAppError::Database("connection refused".to_string())),
        });
    }

    // ---- every other PayrollAppError variant that needs no database id ----

    #[test]
    fn employer_name_cannot_be_empty() {
        check(
            PayrollAppError::EmployerNameCannotBeEmpty,
            StatusCode::BAD_REQUEST,
            "employer_name_cannot_be_empty",
            None,
        );
    }

    #[test]
    fn employer_not_found() {
        let id = EmployerId::new("employer-1");
        check(
            PayrollAppError::EmployerNotFound(id.clone()),
            StatusCode::NOT_FOUND,
            "employer_not_found",
            Some(json!({ "employerId": id.to_string() })),
        );
    }

    #[test]
    fn employment_not_found() {
        let id = EmploymentId::new("employment-1");
        check(
            PayrollAppError::EmploymentNotFound(id.clone()),
            StatusCode::NOT_FOUND,
            "employment_not_found",
            Some(json!({ "employmentId": id.to_string() })),
        );
    }

    #[test]
    fn person_not_found() {
        let id = PersonId::new("person-1");
        check(
            PayrollAppError::PersonNotFound(id.clone()),
            StatusCode::NOT_FOUND,
            "person_not_found",
            Some(json!({ "personId": id.to_string() })),
        );
    }

    #[test]
    fn person_full_name_cannot_be_empty() {
        check(
            PayrollAppError::PersonFullNameCannotBeEmpty,
            StatusCode::BAD_REQUEST,
            "person_full_name_cannot_be_empty",
            None,
        );
    }

    #[test]
    fn employment_is_void() {
        let id = EmploymentId::new("employment-1");
        check(
            PayrollAppError::EmploymentIsVoid(id.clone()),
            StatusCode::CONFLICT,
            "employment_is_void",
            Some(json!({ "employmentId": id.to_string() })),
        );
    }

    #[test]
    fn employment_ends_before_it_starts() {
        check(
            PayrollAppError::EmploymentEndsBeforeItStarts {
                start_date: date(2026, 3, 10),
                end_date: date(2026, 3, 1),
            },
            StatusCode::BAD_REQUEST,
            "employment_ends_before_it_starts",
            Some(json!({ "startDate": "2026-03-10", "endDate": "2026-03-01" })),
        );
    }

    #[test]
    fn no_compensation_terms_in_force() {
        let id = EmploymentId::new("employment-1");
        check(
            PayrollAppError::NoCompensationTermsInForce(id.clone()),
            StatusCode::UNPROCESSABLE_ENTITY,
            "no_compensation_terms_in_force",
            Some(json!({ "employmentId": id.to_string() })),
        );
    }

    #[test]
    fn prior_employment_declaration_cannot_be_unknown() {
        check(
            PayrollAppError::PriorEmploymentDeclarationCannotBeUnknown,
            StatusCode::BAD_REQUEST,
            "prior_employment_declaration_cannot_be_unknown",
            None,
        );
    }

    #[test]
    fn unsupported_deduction_declaration_cannot_be_unknown() {
        check(
            PayrollAppError::UnsupportedDeductionDeclarationCannotBeUnknown,
            StatusCode::BAD_REQUEST,
            "unsupported_deduction_declaration_cannot_be_unknown",
            None,
        );
    }

    #[test]
    fn salt_coverage_start_not_a_period_end() {
        check(
            PayrollAppError::SaltCoverageStartNotAPeriodEnd {
                salt_coverage_start: date(2026, 3, 15),
            },
            StatusCode::UNPROCESSABLE_ENTITY,
            "salt_coverage_start_not_a_period_end",
            Some(json!({ "saltCoverageStart": "2026-03-15" })),
        );
    }

    #[test]
    fn salt_coverage_start_outside_tax_year() {
        check(
            PayrollAppError::SaltCoverageStartOutsideTaxYear {
                salt_coverage_start: date(2026, 3, 31),
                tax_year: TaxYear::starting(2025),
            },
            StatusCode::UNPROCESSABLE_ENTITY,
            "salt_coverage_start_outside_tax_year",
            Some(json!({ "saltCoverageStart": "2026-03-31", "taxYear": 2025 })),
        );
    }

    #[test]
    fn salt_coverage_start_before_employment_is_payable() {
        check(
            PayrollAppError::SaltCoverageStartBeforeEmploymentIsPayable {
                salt_coverage_start: date(2026, 3, 31),
                first_payable_period_end: date(2026, 4, 30),
            },
            StatusCode::UNPROCESSABLE_ENTITY,
            "salt_coverage_start_before_employment_is_payable",
            Some(json!({
                "saltCoverageStart": "2026-03-31",
                "firstPayablePeriodEnd": "2026-04-30",
            })),
        );
    }

    #[test]
    fn opening_balance_figures_over_an_empty_covered_span() {
        check(
            PayrollAppError::OpeningBalanceFiguresOverAnEmptyCoveredSpan {
                salt_coverage_start: date(2026, 3, 31),
            },
            StatusCode::UNPROCESSABLE_ENTITY,
            "opening_balance_figures_over_an_empty_covered_span",
            Some(json!({ "saltCoverageStart": "2026-03-31" })),
        );
    }

    #[test]
    fn pay_period_not_generated_by_the_pay_schedule() {
        check(
            PayrollAppError::PayPeriodNotGeneratedByThePaySchedule {
                period: PayPeriod::new(date(2026, 3, 5), date(2026, 4, 4)).unwrap(),
                schedules_period: period(),
            },
            StatusCode::UNPROCESSABLE_ENTITY,
            "pay_period_not_generated_by_the_pay_schedule",
            Some(json!({
                "periodStart": "2026-03-05",
                "periodEnd": "2026-04-04",
                "schedulesPeriodStart": "2026-03-01",
                "schedulesPeriodEnd": "2026-03-31",
            })),
        );
    }

    #[test]
    fn removal_reason_cannot_be_empty() {
        check(
            PayrollAppError::RemovalReasonCannotBeEmpty,
            StatusCode::BAD_REQUEST,
            "removal_reason_cannot_be_empty",
            None,
        );
    }

    #[test]
    fn basic_pay_cannot_be_set_as_an_earning() {
        check(
            PayrollAppError::BasicPayCannotBeSetAsAnEarning,
            StatusCode::UNPROCESSABLE_ENTITY,
            "basic_pay_cannot_be_set_as_an_earning",
            None,
        );
    }

    /// #49's rule that a raw frozen snapshot is never a response body
    /// reaches the `message` too: the mismatch `Display` diffs the frozen
    /// `PayrollInput` field by field, and none of those field names may
    /// cross the wire. The Employment that moved is still named, in
    /// `details`, and the recovery §0.26 states is the whole message.
    #[tokio::test]
    async fn a_finalization_mismatch_never_spells_out_the_frozen_snapshot() {
        let (approved, _, _) = a_calculable_input_rules_and_calculation();
        let current = an_input_with_earnings(vec![Earning::TaxableAllowance(
            Money::from_cents(50_000).unwrap(),
        )]);
        assert_ne!(approved, current, "the two inputs must actually differ");

        let api_err: ApiError = PayrollAppError::FinalizationInputMismatch {
            employment_id: EmploymentId::new("employment-1"),
            approved: Box::new(approved),
            current: Box::new(current),
        }
        .into();

        let response = api_err.into_response();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("read the response body");
        let body = String::from_utf8(bytes.to_vec()).expect("the body is UTF-8 JSON");

        for frozen_field in ["earnings", "year_to_date", "compensation_terms", "approved"] {
            assert!(
                !body.contains(frozen_field),
                "the frozen snapshot's field {frozen_field} must not reach the body, got {body}"
            );
        }
        assert!(body.contains("calculate it again"), "got {body}");
        assert!(body.contains("employment-1"), "got {body}");
    }

    #[test]
    fn finalization_input_mismatch_names_the_employment() {
        let (input, _, _) = a_calculable_input_rules_and_calculation();
        let employment_id = EmploymentId::new("employment-1");
        check(
            PayrollAppError::FinalizationInputMismatch {
                employment_id: employment_id.clone(),
                approved: Box::new(input.clone()),
                current: Box::new(input),
            },
            StatusCode::CONFLICT,
            "finalization_input_mismatch",
            Some(json!({ "employmentId": employment_id.to_string() })),
        );
    }

    #[test]
    fn finalization_rules_mismatch_names_the_employment() {
        let (_, rules, _) = a_calculable_input_rules_and_calculation();
        let employment_id = EmploymentId::new("employment-1");
        check(
            PayrollAppError::FinalizationRulesMismatch {
                employment_id: employment_id.clone(),
                approved: Box::new(rules.clone()),
                current: Box::new(rules),
            },
            StatusCode::CONFLICT,
            "finalization_rules_mismatch",
            Some(json!({ "employmentId": employment_id.to_string() })),
        );
    }

    #[test]
    fn finalization_calculation_mismatch_names_the_employment() {
        let (_, _, calculation) = a_calculable_input_rules_and_calculation();
        let employment_id = EmploymentId::new("employment-1");
        check(
            PayrollAppError::FinalizationCalculationMismatch {
                employment_id: employment_id.clone(),
                approved: Box::new(calculation.clone()),
                current: Box::new(calculation),
            },
            StatusCode::CONFLICT,
            "finalization_calculation_mismatch",
            Some(json!({ "employmentId": employment_id.to_string() })),
        );
    }

    #[test]
    fn reversal_reason_cannot_be_empty() {
        check(
            PayrollAppError::ReversalReasonCannotBeEmpty,
            StatusCode::BAD_REQUEST,
            "reversal_reason_cannot_be_empty",
            None,
        );
    }

    #[test]
    fn opening_balance_frozen_by_finalization() {
        let employment_id = EmploymentId::new("employment-1");
        check(
            PayrollAppError::OpeningBalanceFrozenByFinalization {
                employment_id: employment_id.clone(),
                tax_year: TaxYear::starting(2026),
            },
            StatusCode::CONFLICT,
            "opening_balance_frozen_by_finalization",
            Some(json!({ "employmentId": employment_id.to_string(), "taxYear": 2026 })),
        );
    }

    #[test]
    fn prior_employment_frozen_by_finalization() {
        let employment_id = EmploymentId::new("employment-1");
        check(
            PayrollAppError::PriorEmploymentFrozenByFinalization {
                employment_id: employment_id.clone(),
                tax_year: TaxYear::starting(2026),
            },
            StatusCode::CONFLICT,
            "prior_employment_frozen_by_finalization",
            Some(json!({ "employmentId": employment_id.to_string(), "taxYear": 2026 })),
        );
    }

    #[test]
    fn pay_schedule_frozen_by_finalization() {
        let employer_id = EmployerId::new("employer-1");
        check(
            PayrollAppError::PayScheduleFrozenByFinalization {
                employer_id: employer_id.clone(),
                tax_year: TaxYear::starting(2026),
            },
            StatusCode::CONFLICT,
            "pay_schedule_frozen_by_finalization",
            Some(json!({ "employerId": employer_id.to_string(), "taxYear": 2026 })),
        );
    }

    #[test]
    fn pay_schedule_change_blocked_by_an_open_run() {
        let employer_id = EmployerId::new("employer-1");
        check(
            PayrollAppError::PayScheduleChangeBlockedByAnOpenRun {
                employer_id: employer_id.clone(),
                period_end: date(2026, 3, 31),
            },
            StatusCode::CONFLICT,
            "pay_schedule_change_blocked_by_an_open_run",
            Some(json!({ "employerId": employer_id.to_string(), "periodEnd": "2026-03-31" })),
        );
    }

    #[test]
    fn pay_schedule_moved_within_tax_year() {
        let employer_id = EmployerId::new("employer-1");
        check(
            PayrollAppError::PayScheduleMovedWithinTaxYear {
                employer_id: employer_id.clone(),
                tax_year: TaxYear::starting(2026),
                finalized_period_end: date(2026, 3, 31),
            },
            StatusCode::CONFLICT,
            "pay_schedule_moved_within_tax_year",
            Some(json!({
                "employerId": employer_id.to_string(),
                "taxYear": 2026,
                "finalizedPeriodEnd": "2026-03-31",
            })),
        );
    }

    #[test]
    fn pay_schedule_change_would_strand_a_stored_boundary_names_every_kind_of_fact() {
        let employer_id = EmployerId::new("employer-1");
        for (fact, code) in [
            (
                ScheduleBoundedFact::SaltCoverageStart,
                "salt_coverage_start",
            ),
            (
                ScheduleBoundedFact::CompensationTermsEffectiveFrom,
                "compensation_terms_effective_from",
            ),
            (
                ScheduleBoundedFact::UnsupportedDeductionEffectiveFrom,
                "unsupported_deduction_effective_from",
            ),
        ] {
            check(
                PayrollAppError::PayScheduleChangeWouldStrandAStoredBoundary {
                    employer_id: employer_id.clone(),
                    fact,
                    boundary: date(2026, 3, 15),
                },
                StatusCode::CONFLICT,
                "pay_schedule_change_would_strand_a_stored_boundary",
                Some(json!({
                    "employerId": employer_id.to_string(),
                    "fact": code,
                    "boundary": "2026-03-15",
                })),
            );
        }
    }

    #[test]
    fn tax_year_outside_representable_calendar() {
        check(
            PayrollAppError::TaxYearOutsideRepresentableCalendar {
                tax_year: TaxYear::starting(999_999),
            },
            StatusCode::UNPROCESSABLE_ENTITY,
            "tax_year_outside_representable_calendar",
            Some(json!({ "taxYear": 999_999 })),
        );
    }

    #[test]
    fn preceding_period_unresolved() {
        let employment_id = EmploymentId::new("employment-1");
        check(
            PayrollAppError::PrecedingPeriodUnresolved {
                employment_id: employment_id.clone(),
                period: period(),
            },
            StatusCode::CONFLICT,
            "preceding_period_unresolved",
            Some(json!({
                "employmentId": employment_id.to_string(),
                "periodStart": "2026-03-01",
                "periodEnd": "2026-03-31",
            })),
        );
    }

    #[test]
    fn correction_reason_cannot_be_empty() {
        check(
            PayrollAppError::CorrectionReasonCannotBeEmpty,
            StatusCode::BAD_REQUEST,
            "correction_reason_cannot_be_empty",
            None,
        );
    }

    #[test]
    fn correction_employment_belongs_to_a_different_employer() {
        let employment_id = EmploymentId::new("employment-1");
        let employer_id = EmployerId::new("employer-1");
        check(
            PayrollAppError::CorrectionEmploymentBelongsToADifferentEmployer {
                employment_id: employment_id.clone(),
                employer_id: employer_id.clone(),
            },
            StatusCode::NOT_FOUND,
            "correction_employment_belongs_to_a_different_employer",
            Some(json!({
                "employmentId": employment_id.to_string(),
                "employerId": employer_id.to_string(),
            })),
        );
    }

    #[test]
    fn correction_lineage_not_legitimate() {
        let employment_id = EmploymentId::new("employment-1");
        check(
            PayrollAppError::CorrectionLineageNotLegitimate {
                employment_id: employment_id.clone(),
                period: period(),
            },
            StatusCode::CONFLICT,
            "correction_lineage_not_legitimate",
            Some(json!({
                "employmentId": employment_id.to_string(),
                "periodStart": "2026-03-01",
                "periodEnd": "2026-03-31",
            })),
        );
    }

    #[test]
    fn correction_period_already_has_a_live_payroll() {
        let employment_id = EmploymentId::new("employment-1");
        check(
            PayrollAppError::CorrectionPeriodAlreadyHasALivePayroll {
                employment_id: employment_id.clone(),
                period: period(),
            },
            StatusCode::CONFLICT,
            "correction_period_already_has_a_live_payroll",
            Some(json!({
                "employmentId": employment_id.to_string(),
                "periodStart": "2026-03-01",
                "periodEnd": "2026-03-31",
            })),
        );
    }

    #[test]
    fn compensation_terms_correction_reason_cannot_be_empty() {
        check(
            PayrollAppError::CompensationTermsCorrectionReasonCannotBeEmpty,
            StatusCode::BAD_REQUEST,
            "compensation_terms_correction_reason_cannot_be_empty",
            None,
        );
    }

    #[test]
    fn no_compensation_terms_row_at() {
        let employment_id = EmploymentId::new("employment-1");
        check(
            PayrollAppError::NoCompensationTermsRowAt {
                employment_id: employment_id.clone(),
                effective_from: date(2026, 3, 1),
            },
            StatusCode::NOT_FOUND,
            "no_compensation_terms_row_at",
            Some(json!({
                "employmentId": employment_id.to_string(),
                "effectiveFrom": "2026-03-01",
            })),
        );
    }

    #[test]
    fn unsupported_deduction_declaration_reason_cannot_be_empty() {
        check(
            PayrollAppError::UnsupportedDeductionDeclarationReasonCannotBeEmpty,
            StatusCode::BAD_REQUEST,
            "unsupported_deduction_declaration_reason_cannot_be_empty",
            None,
        );
    }

    #[test]
    fn master_data_divergence_not_acknowledged_carries_real_dates() {
        let employment_id = EmploymentId::new("employment-1");
        let diverging_periods = vec![
            period(),
            PayPeriod::new(date(2026, 4, 1), date(2026, 4, 30)).unwrap(),
        ];
        check(
            PayrollAppError::MasterDataDivergenceNotAcknowledged {
                employment_id: employment_id.clone(),
                diverging_periods,
            },
            StatusCode::CONFLICT,
            "master_data_divergence_not_acknowledged",
            Some(json!({
                "employmentId": employment_id.to_string(),
                "divergingPeriods": [
                    { "start": "2026-03-01", "end": "2026-03-31" },
                    { "start": "2026-04-01", "end": "2026-04-30" },
                ],
            })),
        );
    }

    #[test]
    fn compensation_terms_already_exist_at() {
        let employment_id = EmploymentId::new("employment-1");
        check(
            PayrollAppError::CompensationTermsAlreadyExistAt {
                employment_id: employment_id.clone(),
                effective_from: date(2026, 3, 1),
            },
            StatusCode::CONFLICT,
            "compensation_terms_already_exist_at",
            Some(json!({
                "employmentId": employment_id.to_string(),
                "effectiveFrom": "2026-03-01",
            })),
        );
    }

    #[test]
    fn operator_email_cannot_be_empty() {
        check(
            PayrollAppError::OperatorEmailCannotBeEmpty,
            StatusCode::BAD_REQUEST,
            "operator_email_cannot_be_empty",
            None,
        );
    }

    #[test]
    fn operator_display_name_cannot_be_empty() {
        check(
            PayrollAppError::OperatorDisplayNameCannotBeEmpty,
            StatusCode::BAD_REQUEST,
            "operator_display_name_cannot_be_empty",
            None,
        );
    }

    #[test]
    fn operator_password_too_short() {
        check(
            PayrollAppError::OperatorPasswordTooShort { minimum: 12 },
            StatusCode::BAD_REQUEST,
            "operator_password_too_short",
            Some(json!({ "minimum": 12 })),
        );
    }

    #[test]
    fn operator_password_too_long() {
        check(
            PayrollAppError::OperatorPasswordTooLong { maximum: 256 },
            StatusCode::BAD_REQUEST,
            "operator_password_too_long",
            Some(json!({ "maximum": 256 })),
        );
    }

    #[test]
    fn operator_email_already_in_use() {
        check(
            PayrollAppError::OperatorEmailAlreadyInUse,
            StatusCode::CONFLICT,
            "operator_email_already_in_use",
            None,
        );
    }

    #[test]
    fn operator_credential_invalid_agrees_with_sessions_own_mapping() {
        check(
            PayrollAppError::OperatorCredentialInvalid,
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            None,
        );
    }

    #[test]
    fn bootstrap_operator_already_exists() {
        check(
            PayrollAppError::BootstrapOperatorAlreadyExists,
            StatusCode::CONFLICT,
            "bootstrap_operator_already_exists",
            None,
        );
    }

    #[test]
    fn bootstrap_operator_table_busy() {
        check(
            PayrollAppError::BootstrapOperatorTableBusy,
            StatusCode::CONFLICT,
            "bootstrap_operator_table_busy",
            None,
        );
    }

    #[test]
    fn bootstrap_period_end_day_invalid() {
        check(
            PayrollAppError::BootstrapPeriodEndDayInvalid { day: 30 },
            StatusCode::BAD_REQUEST,
            "bootstrap_period_end_day_invalid",
            Some(json!({ "day": 30 })),
        );
    }

    // ---- every PayrollError variant, all 422 ----

    #[test]
    fn compensation_terms_do_not_cover_period() {
        check_payroll_error(
            PayrollError::CompensationTermsDoNotCoverPeriod,
            "compensation_terms_do_not_cover_period",
            None,
        );
    }

    #[test]
    fn pay_period_not_on_the_employers_schedule() {
        check_payroll_error(
            PayrollError::PayPeriodNotOnTheEmployersSchedule { expected: period() },
            "pay_period_not_on_the_employers_schedule",
            Some(json!({ "expectedPeriodStart": "2026-03-01", "expectedPeriodEnd": "2026-03-31" })),
        );
    }

    #[test]
    fn pay_schedule_outside_representable_calendar() {
        check_payroll_error(
            PayrollError::PayScheduleOutsideRepresentableCalendar {
                date: date(2026, 3, 15),
            },
            "pay_schedule_outside_representable_calendar",
            Some(json!({ "date": "2026-03-15" })),
        );
    }

    #[test]
    fn effective_from_not_a_period_start() {
        check_payroll_error(
            PayrollError::EffectiveFromNotAPeriodStart {
                next_valid_effective_from: date(2026, 4, 1),
            },
            "effective_from_not_a_period_start",
            Some(json!({ "nextValidEffectiveFrom": "2026-04-01" })),
        );
    }

    #[test]
    fn contradictory_employment_dates() {
        check_payroll_error(
            PayrollError::ContradictoryEmploymentDates,
            "contradictory_employment_dates",
            None,
        );
    }

    #[test]
    fn employment_does_not_overlap_period() {
        check_payroll_error(
            PayrollError::EmploymentDoesNotOverlapPeriod,
            "employment_does_not_overlap_period",
            None,
        );
    }

    #[test]
    fn duplicate_basic_pay_line() {
        check_payroll_error(
            PayrollError::DuplicateBasicPayLine,
            "duplicate_basic_pay_line",
            None,
        );
    }

    #[test]
    fn prior_paye_exceeds_recalculated_liability() {
        check_payroll_error(
            PayrollError::PriorPayeExceedsRecalculatedLiability,
            "prior_paye_exceeds_recalculated_liability",
            None,
        );
    }

    #[test]
    fn deductions_exceed_gross_remuneration() {
        check_payroll_error(
            PayrollError::DeductionsExceedGrossRemuneration,
            "deductions_exceed_gross_remuneration",
            None,
        );
    }

    #[test]
    fn amount_overflow() {
        check_payroll_error(PayrollError::AmountOverflow, "amount_overflow", None);
    }

    #[test]
    fn no_paye_table_covers_date() {
        check_payroll_error(
            PayrollError::NoPayeTableCoversDate {
                date: date(2026, 3, 31),
            },
            "no_paye_table_covers_date",
            Some(json!({ "date": "2026-03-31" })),
        );
    }

    #[test]
    fn overlapping_paye_tables() {
        check_payroll_error(
            PayrollError::OverlappingPayeTables {
                first: PayeTableId::new("paye-1"),
                second: PayeTableId::new("paye-2"),
            },
            "overlapping_paye_tables",
            Some(json!({ "first": "paye-1", "second": "paye-2" })),
        );
    }

    #[test]
    fn no_ssc_ruleset_covers_date() {
        check_payroll_error(
            PayrollError::NoSscRulesetCoversDate {
                date: date(2026, 3, 31),
            },
            "no_ssc_ruleset_covers_date",
            Some(json!({ "date": "2026-03-31" })),
        );
    }

    #[test]
    fn overlapping_ssc_rulesets() {
        check_payroll_error(
            PayrollError::OverlappingSscRulesets {
                first: SscRulesId::new("ssc-1"),
                second: SscRulesId::new("ssc-2"),
            },
            "overlapping_ssc_rulesets",
            Some(json!({ "first": "ssc-1", "second": "ssc-2" })),
        );
    }

    #[test]
    fn paye_table_does_not_cover_period() {
        check_payroll_error(
            PayrollError::PayeTableDoesNotCoverPeriod {
                table: PayeTableId::new("paye-1"),
                period_end: date(2026, 3, 31),
            },
            "paye_table_does_not_cover_period",
            Some(json!({ "table": "paye-1", "periodEnd": "2026-03-31" })),
        );
    }

    #[test]
    fn ssc_ruleset_does_not_cover_period() {
        check_payroll_error(
            PayrollError::SscRulesetDoesNotCoverPeriod {
                ruleset: SscRulesId::new("ssc-1"),
                period_end: date(2026, 3, 31),
            },
            "ssc_ruleset_does_not_cover_period",
            Some(json!({ "ruleset": "ssc-1", "periodEnd": "2026-03-31" })),
        );
    }

    #[test]
    fn wrong_tax_year_for_period() {
        check_payroll_error(
            PayrollError::WrongTaxYearForPeriod {
                expected: TaxYear::starting(2026),
                supplied: TaxYear::starting(2025),
            },
            "wrong_tax_year_for_period",
            Some(json!({ "expectedTaxYear": 2026, "suppliedTaxYear": 2025 })),
        );
    }

    #[test]
    fn unsupported_deduction_status_unknown() {
        check_payroll_error(
            PayrollError::UnsupportedDeductionStatusUnknown,
            "unsupported_deduction_status_unknown",
            None,
        );
    }

    #[test]
    fn unsupported_deductions_present_names_every_kind() {
        let kinds = UnsupportedDeductionKinds::new(vec![
            payroll::UnsupportedDeductionKind::ApprovedPensionFund,
            payroll::UnsupportedDeductionKind::ProvidentFund,
        ])
        .unwrap();
        check_payroll_error(
            PayrollError::UnsupportedDeductionsPresent { kinds },
            "unsupported_deductions_present",
            Some(json!({ "kinds": ["approved_pension_fund", "provident_fund"] })),
        );
    }

    #[test]
    fn unsupported_deduction_kind_codes_parse_back_to_the_same_kind() {
        for kind in [
            payroll::UnsupportedDeductionKind::ApprovedPensionFund,
            payroll::UnsupportedDeductionKind::ProvidentFund,
            payroll::UnsupportedDeductionKind::RetirementAnnuityFund,
            payroll::UnsupportedDeductionKind::EducationPolicy,
        ] {
            assert_eq!(
                parse_unsupported_deduction_kind(unsupported_deduction_kind_code(&kind)),
                Some(kind)
            );
        }
        assert_eq!(parse_unsupported_deduction_kind("not-a-real-kind"), None);
    }

    #[test]
    fn prior_employment_unknown() {
        check_payroll_error(
            PayrollError::PriorEmploymentUnknown,
            "prior_employment_unknown",
            None,
        );
    }

    #[test]
    fn prior_employment_treatment_unconfirmed_carries_the_figures_in_cents() {
        let figures = PriorEmploymentFigures::new(
            Money::from_cents(100_000).unwrap(),
            Money::from_cents(20_000).unwrap(),
        );
        check_payroll_error(
            PayrollError::PriorEmploymentPresent { figures },
            "prior_employment_treatment_unconfirmed",
            Some(json!({ "taxableRemunerationCents": 100_000, "payeCents": 20_000 })),
        );
    }

    // ---- variants that need an id only `payroll-app` itself can mint ----

    fn test_database_config() -> DatabaseConfig {
        let url = std::env::var("DATABASE_URL").expect(
            "DATABASE_URL must name an already-migrated database for salt-server's own tests \
             (see crates/salt-server/src/authorized_employer.rs)",
        );
        DatabaseConfig {
            url,
            max_connections: 2,
            acquire_timeout: std::time::Duration::from_secs(10),
            idle_timeout: None,
        }
    }

    async fn test_db() -> SaltDatabase {
        SaltDatabase::connect(&test_database_config())
            .await
            .expect("connect to the local, migrated test database (see AGENTS.md)")
    }

    fn unique_email() -> String {
        format!("payroll-error-{}@example.com", uuid::Uuid::new_v4())
    }

    /// A run and, separately, a genuinely finalized run with one member —
    /// the only way to hold a real [`PayrollRunId`] and [`FinalizedPayrollId`]
    /// from outside `payroll-app` (both mint theirs with a `pub(crate)`
    /// constructor). Every assertion below constructs its own
    /// [`PayrollAppError`] value directly; nothing here exercises the use
    /// cases that would actually raise them for real.
    #[tokio::test]
    async fn variants_naming_a_payroll_app_minted_id_map_correctly() {
        let db = test_db().await;
        let schedule = || PaySchedule::new(PeriodEndDay::LastDayOfMonth);

        let operator_id: OperatorId =
            payroll_app::create_operator(&db, &unique_email(), "Test Operator", "correct horse")
                .await
                .unwrap();

        let plain_employer_id = payroll_app::create_employer(&db, "Employer", schedule(), "actor")
            .await
            .unwrap();
        let unfinalized_run_id: PayrollRunId = payroll_app::create_ordinary_payroll_run(
            &db,
            &plain_employer_id,
            period(),
            date(2026, 4, 5),
            "actor",
        )
        .await
        .unwrap();

        let finalized_employer_id =
            payroll_app::create_employer(&db, "Employer 2", schedule(), "actor")
                .await
                .unwrap();
        let (_, employment_id) = payroll_app::create_employment(
            &db,
            &finalized_employer_id,
            payroll_app::EmploymentPerson::New("Test Person".to_string()),
            period().start(),
            None,
            "actor",
        )
        .await
        .unwrap();
        payroll_app::record_compensation_terms(
            &db,
            &employment_id,
            period().start(),
            Money::from_cents(1_500_000).unwrap(),
            &[],
            "",
            "actor",
        )
        .await
        .unwrap();
        payroll_app::declare_prior_employment(
            &db,
            &employment_id,
            TaxYear::for_period_end(period().end()),
            PriorEmployment::None,
            "actor",
        )
        .await
        .unwrap();
        payroll_app::declare_unsupported_deduction_status(
            &db,
            &employment_id,
            period().start(),
            UnsupportedDeductionStatus::ConfirmedNone,
            &[],
            "a reason",
            "actor",
        )
        .await
        .unwrap();
        let finalized_run_id = payroll_app::create_ordinary_payroll_run(
            &db,
            &finalized_employer_id,
            period(),
            date(2026, 4, 5),
            "actor",
        )
        .await
        .unwrap();
        payroll_app::calculate_payroll_run(&db, &finalized_run_id, "calculator")
            .await
            .unwrap();
        let outcome = payroll_app::finalize_payroll_run(&db, &finalized_run_id, "finalizer")
            .await
            .unwrap();
        let finalized_payroll_id: FinalizedPayrollId = outcome.finalized[0].1.clone();

        check(
            PayrollAppError::PayrollRunNotFound(unfinalized_run_id.clone()),
            StatusCode::NOT_FOUND,
            "payroll_run_not_found",
            Some(json!({ "payrollRunId": unfinalized_run_id.to_string() })),
        );
        check(
            PayrollAppError::PayrollRunIsNotOrdinary(unfinalized_run_id.clone()),
            StatusCode::CONFLICT,
            "payroll_run_is_not_ordinary",
            Some(json!({ "payrollRunId": unfinalized_run_id.to_string() })),
        );
        check(
            PayrollAppError::PayrollRunNotCalculated(unfinalized_run_id.clone()),
            StatusCode::CONFLICT,
            "payroll_run_not_calculated",
            Some(json!({ "payrollRunId": unfinalized_run_id.to_string() })),
        );
        // A vacuous run: finalized with no active member, so there is
        // nothing to name and both keys say so.
        check(
            PayrollAppError::PayrollRunAlreadyFinalized {
                payroll_run_id: unfinalized_run_id.clone(),
                finalized_payrolls: Vec::new(),
            },
            StatusCode::CONFLICT,
            "payroll_run_already_finalized",
            Some(json!({
                "payrollRunId": unfinalized_run_id.to_string(),
                "finalizedPayrollId": null,
                "finalizedPayrolls": [],
            })),
        );
        // One member: `finalizedPayrollId` is the id a client navigates to
        // directly (§0.28).
        check(
            PayrollAppError::PayrollRunAlreadyFinalized {
                payroll_run_id: finalized_run_id.clone(),
                finalized_payrolls: vec![(employment_id.clone(), finalized_payroll_id.clone())],
            },
            StatusCode::CONFLICT,
            "payroll_run_already_finalized",
            Some(json!({
                "payrollRunId": finalized_run_id.to_string(),
                "finalizedPayrollId": finalized_payroll_id.to_string(),
                "finalizedPayrolls": [{
                    "employmentId": employment_id.to_string(),
                    "finalizedPayrollId": finalized_payroll_id.to_string(),
                }],
            })),
        );
        // Two members: each finalizes into its own row, so there is no
        // single id to shortcut to, and every one is still named.
        let second_employment_id = EmploymentId::new("employment-2");
        let second_finalized_payroll_id = finalized_payroll_id.clone();
        check(
            PayrollAppError::PayrollRunAlreadyFinalized {
                payroll_run_id: finalized_run_id.clone(),
                finalized_payrolls: vec![
                    (employment_id.clone(), finalized_payroll_id.clone()),
                    (
                        second_employment_id.clone(),
                        second_finalized_payroll_id.clone(),
                    ),
                ],
            },
            StatusCode::CONFLICT,
            "payroll_run_already_finalized",
            Some(json!({
                "payrollRunId": finalized_run_id.to_string(),
                "finalizedPayrollId": null,
                "finalizedPayrolls": [
                    {
                        "employmentId": employment_id.to_string(),
                        "finalizedPayrollId": finalized_payroll_id.to_string(),
                    },
                    {
                        "employmentId": second_employment_id.to_string(),
                        "finalizedPayrollId": second_finalized_payroll_id.to_string(),
                    },
                ],
            })),
        );
        check(
            PayrollAppError::EmploymentNotAnActiveRunMember {
                payroll_run_id: unfinalized_run_id.clone(),
                employment_id: employment_id.clone(),
            },
            StatusCode::CONFLICT,
            "employment_not_an_active_run_member",
            Some(json!({
                "payrollRunId": unfinalized_run_id.to_string(),
                "employmentId": employment_id.to_string(),
            })),
        );
        check(
            PayrollAppError::PayrollRunIsNotCorrection(unfinalized_run_id.clone()),
            StatusCode::CONFLICT,
            "payroll_run_is_not_correction",
            Some(json!({ "payrollRunId": unfinalized_run_id.to_string() })),
        );
        check(
            PayrollAppError::CorrectionRunAlreadyHasAnEmployment(unfinalized_run_id.clone()),
            StatusCode::CONFLICT,
            "correction_run_already_has_an_employment",
            Some(json!({ "payrollRunId": unfinalized_run_id.to_string() })),
        );
        check(
            PayrollAppError::FinalizedPayrollNotFound(finalized_payroll_id.clone()),
            StatusCode::NOT_FOUND,
            "finalized_payroll_not_found",
            Some(json!({ "finalizedPayrollId": finalized_payroll_id.to_string() })),
        );
        check(
            PayrollAppError::FinalizedPayrollAlreadyReversed(finalized_payroll_id.clone()),
            StatusCode::CONFLICT,
            "finalized_payroll_already_reversed",
            Some(json!({ "finalizedPayrollId": finalized_payroll_id.to_string() })),
        );
        check(
            PayrollAppError::CorrectionTargetDoesNotMatch {
                payroll_run_id: unfinalized_run_id.clone(),
                finalized_payroll_id: finalized_payroll_id.clone(),
            },
            StatusCode::CONFLICT,
            "correction_target_does_not_match",
            Some(json!({
                "payrollRunId": unfinalized_run_id.to_string(),
                "finalizedPayrollId": finalized_payroll_id.to_string(),
            })),
        );
        check(
            PayrollAppError::CorrectionTargetNotReversed(finalized_payroll_id.clone()),
            StatusCode::CONFLICT,
            "correction_target_not_reversed",
            Some(json!({ "finalizedPayrollId": finalized_payroll_id.to_string() })),
        );
        check(
            PayrollAppError::CorrectionTargetAlreadyReplaced(finalized_payroll_id.clone()),
            StatusCode::CONFLICT,
            "correction_target_already_replaced",
            Some(json!({ "finalizedPayrollId": finalized_payroll_id.to_string() })),
        );
        check(
            PayrollAppError::CorrectionLineageOmitsAReversedPredecessor {
                employment_id: employment_id.clone(),
                period: period(),
                finalized_payroll_id: finalized_payroll_id.clone(),
            },
            StatusCode::CONFLICT,
            "correction_lineage_omits_a_reversed_predecessor",
            Some(json!({
                "employmentId": employment_id.to_string(),
                "periodStart": "2026-03-01",
                "periodEnd": "2026-03-31",
                "finalizedPayrollId": finalized_payroll_id.to_string(),
            })),
        );
        check(
            PayrollAppError::OperatorNotFound(operator_id.clone()),
            StatusCode::NOT_FOUND,
            "operator_not_found",
            Some(json!({ "operatorId": operator_id.to_string() })),
        );
        check(
            PayrollAppError::OperatorAlreadyDisabled(operator_id.clone()),
            StatusCode::CONFLICT,
            "operator_already_disabled",
            Some(json!({ "operatorId": operator_id.to_string() })),
        );

        let employer_id = EmployerId::new("employer-x");
        check(
            PayrollAppError::EmployerMembershipAlreadyExists {
                operator_id: operator_id.clone(),
                employer_id: employer_id.clone(),
            },
            StatusCode::CONFLICT,
            "employer_membership_already_exists",
            Some(json!({
                "operatorId": operator_id.to_string(),
                "employerId": employer_id.to_string(),
            })),
        );
        check(
            PayrollAppError::EmployerMembershipNotFound {
                operator_id: operator_id.clone(),
                employer_id: employer_id.clone(),
            },
            StatusCode::NOT_FOUND,
            "employer_membership_not_found",
            Some(json!({
                "operatorId": operator_id.to_string(),
                "employerId": employer_id.to_string(),
            })),
        );
        check(
            PayrollAppError::EmployerMembershipAlreadyRevoked {
                operator_id: operator_id.clone(),
                employer_id: employer_id.clone(),
            },
            StatusCode::CONFLICT,
            "employer_membership_already_revoked",
            Some(json!({
                "operatorId": operator_id.to_string(),
                "employerId": employer_id.to_string(),
            })),
        );
    }
}
