use chrono::NaiveDate;
use payroll::{
    EmployerId, EmploymentId, PayPeriod, PayrollCalculation, PayrollError, PayrollInput,
    PayrollRules, TaxYear,
};

use crate::finalize::FinalizedPayrollId;
use crate::payroll_run::PayrollRunId;

/// `payroll-app`'s own error type. It wraps [`PayrollError`] rather than
/// re-exporting it, because "PostgreSQL unavailable" and "run already
/// finalized" are different categories and never share an enum (ADR-0009).
///
/// Wrapping keeps the pure crate's refusals distinguishable by variant
/// while leaving room for the infrastructure and run-state variants this
/// crate will grow. `?` converts a [`PayrollError`] into the [`Payroll`]
/// variant; nothing else does.
///
/// [`Payroll`]: PayrollAppError::Payroll
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayrollAppError {
    /// The pure calculator refused. The refusal is the whole error — this
    /// layer has nothing to add to it.
    Payroll(PayrollError),
    /// PostgreSQL refused the statement, or the connection failed. Never a
    /// domain refusal: a constraint violation surfaces here, not as a
    /// [`Payroll`] variant, even when it happens to guard a rule Rust also
    /// knows about.
    ///
    /// [`Payroll`]: PayrollAppError::Payroll
    Database(String),
    /// No Employer exists with this id.
    EmployerNotFound(EmployerId),
    /// No Employment exists with this id.
    EmploymentNotFound(EmploymentId),
    /// The Employment is void (§4.3). A voided Employment is a recorded
    /// mistake: it never appears in run membership, so no standing fact may
    /// be recorded against it and no calculation input may be read from it.
    EmploymentIsVoid(EmploymentId),
    /// The Employment's `end_date` falls before its `start_date`. A domain
    /// refusal, stated here rather than left to the table's own CHECK, so a
    /// caller is told the dates disagree instead of being handed a database
    /// error.
    EmploymentEndsBeforeItStarts {
        start_date: NaiveDate,
        end_date: NaiveDate,
    },
    /// No `CompensationTerms` row is in force for this Employment as of the
    /// requested date.
    NoCompensationTermsInForce(EmploymentId),
    /// A `PriorEmployment` declaration was given as `Unknown`. `Unknown` is
    /// what an absent row means (§4.5b) — it describes silence, not a fact
    /// to write, so declaring it explicitly is refused rather than stored.
    PriorEmploymentDeclarationCannotBeUnknown,
    /// An `UnsupportedDeductionStatus` declaration was given as `Unknown`,
    /// for the same reason: `Unknown` is what no row in force means (§4.5c),
    /// not a fact a declaration can state.
    UnsupportedDeductionDeclarationCannotBeUnknown,
    /// An `OpeningBalance`'s `SaltCoverageStart` is not a `PayPeriod` end
    /// date the Employer's `PaySchedule` generates (§4.5 guard 1).
    SaltCoverageStartNotAPeriodEnd { salt_coverage_start: NaiveDate },
    /// An `OpeningBalance`'s `SaltCoverageStart` does not fall inside the
    /// row's own `TaxYear` (§4.5 guard 2).
    SaltCoverageStartOutsideTaxYear {
        salt_coverage_start: NaiveDate,
        tax_year: TaxYear,
    },
    /// An `OpeningBalance`'s `SaltCoverageStart` falls before the
    /// Employment's first payable `PayPeriod` end in that `TaxYear` (§4.5
    /// guard 3): Salt cannot claim to have replaced a system for periods in
    /// which the Employment did not exist.
    SaltCoverageStartBeforeEmploymentIsPayable {
        salt_coverage_start: NaiveDate,
        first_payable_period_end: NaiveDate,
    },
    /// Non-zero `OpeningBalance` figures were given over an empty covered
    /// span — `SaltCoverageStart` equal to the Employment's first payable
    /// period end, so there is no pre-Salt period left for the figures to
    /// describe (§4.5 guard 4).
    OpeningBalanceFiguresOverAnEmptyCoveredSpan { salt_coverage_start: NaiveDate },
    /// A PayrollRun was asked for a `PayPeriod` the Employer's own
    /// `PaySchedule` does not generate (§4.2, §4.6). Everything downstream
    /// reads a run's period as one of the schedule's twelve: sequencing
    /// walks back to "the immediately preceding PayPeriod" (§8), the
    /// Ordinary uniqueness index keys on the period end alone (§4.6), and
    /// `calculate` resolves the CompensationTerms in force from the
    /// period's own boundaries. A period the schedule never generates has
    /// no predecessor and no successor, so it is refused at the one point
    /// it enters the system.
    ///
    /// `schedules_period` is the period the schedule does generate around
    /// the requested end date — the one the caller almost certainly meant,
    /// named for the same reason [`PayrollError::EffectiveFromNotAPeriodStart`]
    /// names the next valid date.
    PayPeriodNotGeneratedByThePaySchedule {
        period: PayPeriod,
        schedules_period: PayPeriod,
    },
    /// No PayrollRun exists with this id.
    PayrollRunNotFound(PayrollRunId),
    /// Removing a member is the deliberate omission path of an Ordinary run;
    /// Correction runs have the opposite membership semantics (§4.8).
    PayrollRunIsNotOrdinary(PayrollRunId),
    /// `RemoveEmploymentFromRun` was given a reason that is empty or only
    /// whitespace. Silent omission is the dangerous failure (§4.8) — a
    /// removal is a deliberate, reasoned act, and a blank reason states
    /// nothing while looking like it states something.
    RemovalReasonCannotBeEmpty,
    /// `payroll_run_id` and `employment_id` do not name a live membership:
    /// either the Employment was never proposed into that run, or it was
    /// already removed. The two are not distinguished, for the same reason
    /// `void_employment` does not distinguish "never existed" from
    /// "already void" any further than it needs to — either way there is no
    /// live membership to act on.
    ///
    /// Both `RemoveEmploymentFromRun` and `SetRunEarnings` refuse with it.
    /// Earning lines are a fact about paying this Employment for this
    /// period, so a run that is not paying it has nowhere to put them.
    EmploymentNotAnActiveRunMember {
        payroll_run_id: PayrollRunId,
        employment_id: EmploymentId,
    },
    /// `SetRunEarnings` was given a `BasicPay` line. `calculate` derives
    /// `BasicPay` itself from the Employment's `CompensationTerms` — it is
    /// also the social security base — so a second one supplied as a run
    /// Earning would silently double it (§4.5d).
    BasicPayCannotBeSetAsAnEarning,
    /// Working state — membership, Earnings, the working calculation — was
    /// asked to change on a run that is already `Finalized`. Once history
    /// has been written there is nothing left to overwrite (§4.7). A
    /// `Calculated` run is not refused: editing it reopens it as `Draft`.
    PayrollRunAlreadyFinalized(PayrollRunId),
    /// `FinalizePayrollRun` was asked for a run that is not yet `Calculated`
    /// — still `Draft`, with at least one member unresolved. `Finalized` is
    /// the separate, absolute refusal above.
    PayrollRunNotCalculated(PayrollRunId),
    /// Finalization's rebuild of one member refused before any of the three
    /// comparisons could be made — an Employment voided, a
    /// `CompensationTerms` row withdrawn, a declaration removed since the
    /// run reached `Calculated`, or any refusal `calculate` itself raises.
    ///
    /// The Employment is named alongside the refusal for the same reason
    /// [`crate::PayrollRunCalculationRefusal`] names it and the three
    /// mismatches below do: an Employer told only "PriorEmployment is
    /// Unknown" about a ten-member run has been told nothing they can act
    /// on. The whole run refuses regardless (§5.1) — this says which member
    /// to go and fix.
    FinalizationRebuildRefused {
        employment_id: EmploymentId,
        refusal: Box<PayrollAppError>,
    },
    /// Finalization's reassembled `PayrollInput` no longer equals what the
    /// working calculation approved (§5.2). Carries both so the mismatch is
    /// explainable rather than merely declared — the whole point of
    /// comparing all three rather than the `PayrollCalculation` alone.
    FinalizationInputMismatch {
        employment_id: EmploymentId,
        approved: Box<PayrollInput>,
        current: Box<PayrollInput>,
    },
    /// Finalization's re-resolved `PayrollRules` no longer equal what the
    /// working calculation approved (§5.2) — e.g. a PAYE band corrected
    /// outside the range this Employee reaches, which leaves the money
    /// identical while the frozen rules would differ.
    FinalizationRulesMismatch {
        employment_id: EmploymentId,
        approved: Box<PayrollRules>,
        current: Box<PayrollRules>,
    },
    /// Finalization's recomputed `PayrollCalculation` no longer equals what
    /// the working calculation approved (§5.2).
    FinalizationCalculationMismatch {
        employment_id: EmploymentId,
        approved: Box<PayrollCalculation>,
        current: Box<PayrollCalculation>,
    },
    /// No `FinalizedPayroll` exists with this id.
    FinalizedPayrollNotFound(FinalizedPayrollId),
    /// `ReverseFinalizedPayroll` was asked for a `FinalizedPayroll` that
    /// already has a `Reversal` (§6.1) — `reversal.finalized_payroll_id` is
    /// UNIQUE (migration 0011), and there is no unreversal record type to
    /// undo one with (ADR-0002's rejection, restated in §6.1).
    FinalizedPayrollAlreadyReversed(FinalizedPayrollId),
    /// `ReverseFinalizedPayroll` was given a reason that is empty or only
    /// whitespace. A reversal is a deliberate, attributed act (§6.1), the
    /// same demand `RemovalReasonCannotBeEmpty` makes of a run removal.
    ReversalReasonCannotBeEmpty,
    /// `RecordOpeningBalance` was asked to write or replace a row for an
    /// (Employment, TaxYear) that already has a `FinalizedPayroll` — Live or
    /// reversed (ADR-0013, §4.5). `OpeningBalance` is re-read into every
    /// later period's `YearToDateContext`, so an edit after that point would
    /// re-price already-finalized figures while their frozen snapshots kept
    /// showing the old ones.
    OpeningBalanceFrozenByFinalization {
        employment_id: EmploymentId,
        tax_year: TaxYear,
    },
    /// `DeclarePriorEmployment` was asked to write or replace a row for an
    /// (Employment, TaxYear) that already has a `FinalizedPayroll` — Live or
    /// reversed (ADR-0013, §4.5b). The same reason as
    /// [`OpeningBalanceFrozenByFinalization`]: this fact is re-read into
    /// every later period's `YearToDateContext`.
    PriorEmploymentFrozenByFinalization {
        employment_id: EmploymentId,
        tax_year: TaxYear,
    },
    /// `ChangePaySchedule` was asked to change an Employer whose Employments
    /// already have a `FinalizedPayroll` — Live or reversed — in the given
    /// TaxYear (ADR-0013, §4.2). The guard that keeps a TaxYear at exactly
    /// twelve periods and cumulative PAYE sound.
    PayScheduleFrozenByFinalization {
        employer_id: EmployerId,
        tax_year: TaxYear,
    },
}

impl std::fmt::Display for PayrollAppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Payroll(err) => write!(f, "{err}"),
            Self::Database(message) => write!(f, "database error: {message}"),
            Self::EmployerNotFound(id) => write!(f, "no Employer exists with id {id}"),
            Self::EmploymentNotFound(id) => write!(f, "no Employment exists with id {id}"),
            Self::EmploymentIsVoid(id) => write!(f, "Employment {id} is void"),
            Self::EmploymentEndsBeforeItStarts {
                start_date,
                end_date,
            } => write!(
                f,
                "an Employment ending {end_date} cannot start later, on {start_date}"
            ),
            Self::NoCompensationTermsInForce(id) => {
                write!(f, "no CompensationTerms are in force for Employment {id}")
            }
            Self::PriorEmploymentDeclarationCannotBeUnknown => write!(
                f,
                "a PriorEmployment declaration cannot itself be Unknown; omit the declaration instead"
            ),
            Self::UnsupportedDeductionDeclarationCannotBeUnknown => write!(
                f,
                "an UnsupportedDeductionStatus declaration cannot itself be Unknown; omit the declaration instead"
            ),
            Self::SaltCoverageStartNotAPeriodEnd {
                salt_coverage_start,
            } => write!(
                f,
                "SaltCoverageStart {salt_coverage_start} is not a PayPeriod end date the Employer's PaySchedule generates"
            ),
            Self::SaltCoverageStartOutsideTaxYear {
                salt_coverage_start,
                tax_year,
            } => write!(
                f,
                "SaltCoverageStart {salt_coverage_start} falls outside TaxYear {}",
                tax_year.starting_year()
            ),
            Self::SaltCoverageStartBeforeEmploymentIsPayable {
                salt_coverage_start,
                first_payable_period_end,
            } => write!(
                f,
                "SaltCoverageStart {salt_coverage_start} falls before the Employment's first payable PayPeriod end {first_payable_period_end} in that TaxYear"
            ),
            Self::OpeningBalanceFiguresOverAnEmptyCoveredSpan {
                salt_coverage_start,
            } => write!(
                f,
                "non-zero OpeningBalance figures were given over an empty covered span: SaltCoverageStart {salt_coverage_start} is itself the Employment's first payable PayPeriod end, so no pre-Salt period remains for them to describe"
            ),
            Self::PayPeriodNotGeneratedByThePaySchedule {
                period,
                schedules_period,
            } => write!(
                f,
                "PayPeriod {} to {} is not one the Employer's PaySchedule generates; \
                 the schedule's own period around that end date is {} to {}",
                period.start(),
                period.end(),
                schedules_period.start(),
                schedules_period.end()
            ),
            Self::PayrollRunNotFound(id) => write!(f, "no PayrollRun exists with id {id}"),
            Self::PayrollRunIsNotOrdinary(id) => {
                write!(f, "PayrollRun {id} is not an Ordinary run")
            }
            Self::RemovalReasonCannotBeEmpty => {
                write!(f, "a removal reason must not be empty")
            }
            Self::EmploymentNotAnActiveRunMember {
                payroll_run_id,
                employment_id,
            } => write!(
                f,
                "Employment {employment_id} is not an active member of PayrollRun {payroll_run_id}"
            ),
            Self::BasicPayCannotBeSetAsAnEarning => write!(
                f,
                "BasicPay is derived by calculate() from the compensation terms and cannot be supplied as a run Earning"
            ),
            Self::PayrollRunAlreadyFinalized(id) => {
                write!(f, "PayrollRun {id} is already Finalized")
            }
            Self::PayrollRunNotCalculated(id) => {
                write!(f, "PayrollRun {id} is not Calculated")
            }
            Self::FinalizationRebuildRefused {
                employment_id,
                refusal,
            } => write!(
                f,
                "finalizing Employment {employment_id} refused: {refusal}"
            ),
            Self::FinalizationInputMismatch {
                employment_id,
                approved,
                current,
            } => write!(
                f,
                "finalizing Employment {employment_id} refused: the reassembled PayrollInput no \
                 longer equals what was approved — {}",
                describe_differences(
                    serde_json::to_value(approved).ok(),
                    serde_json::to_value(current).ok(),
                )
            ),
            Self::FinalizationRulesMismatch {
                employment_id,
                approved,
                current,
            } => write!(
                f,
                "finalizing Employment {employment_id} refused: the re-resolved PayrollRules no \
                 longer equal what was approved — {}",
                describe_differences(
                    serde_json::to_value(approved).ok(),
                    serde_json::to_value(current).ok(),
                )
            ),
            Self::FinalizationCalculationMismatch {
                employment_id,
                approved,
                current,
            } => write!(
                f,
                "finalizing Employment {employment_id} refused: the recomputed \
                 PayrollCalculation no longer equals what was approved — {}",
                describe_differences(
                    serde_json::to_value(approved).ok(),
                    serde_json::to_value(current).ok(),
                )
            ),
            Self::FinalizedPayrollNotFound(id) => {
                write!(f, "no FinalizedPayroll exists with id {id}")
            }
            Self::FinalizedPayrollAlreadyReversed(id) => {
                write!(f, "FinalizedPayroll {id} has already been reversed")
            }
            Self::ReversalReasonCannotBeEmpty => {
                write!(f, "a reversal reason must not be empty")
            }
            Self::OpeningBalanceFrozenByFinalization {
                employment_id,
                tax_year,
            } => write!(
                f,
                "OpeningBalance for Employment {employment_id} in TaxYear {} is frozen: \
                 that Employment has already finalized a payroll in that TaxYear",
                tax_year.starting_year()
            ),
            Self::PriorEmploymentFrozenByFinalization {
                employment_id,
                tax_year,
            } => write!(
                f,
                "the PriorEmployment declaration for Employment {employment_id} in TaxYear {} \
                 is frozen: that Employment has already finalized a payroll in that TaxYear",
                tax_year.starting_year()
            ),
            Self::PayScheduleFrozenByFinalization {
                employer_id,
                tax_year,
            } => write!(
                f,
                "Employer {employer_id}'s PaySchedule is frozen for TaxYear {}: a payroll has \
                 already been finalized in that TaxYear",
                tax_year.starting_year()
            ),
        }
    }
}

impl std::error::Error for PayrollAppError {
    /// The wrapped error's own source, never the wrapped error itself.
    ///
    /// `Display` for the `Payroll` variant already prints the
    /// [`PayrollError`]'s message verbatim, so returning that same error as
    /// the source would make every chain-printing reporter — `tracing`,
    /// `anyhow`, a log line walking `source()` — print the one refusal
    /// twice. This is what `thiserror`'s `#[error(transparent)]` does,
    /// written out.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Payroll(err) => err.source(),
            // Same reason as `Payroll` above: `Display` already prints the
            // wrapped refusal's message, so returning it as the source would
            // make every chain-printing reporter say it twice. Whatever sat
            // *below* it is still reachable.
            Self::FinalizationRebuildRefused { refusal, .. } => refusal.source(),
            Self::Database(_)
            | Self::EmployerNotFound(_)
            | Self::EmploymentNotFound(_)
            | Self::EmploymentIsVoid(_)
            | Self::EmploymentEndsBeforeItStarts { .. }
            | Self::NoCompensationTermsInForce(_)
            | Self::PriorEmploymentDeclarationCannotBeUnknown
            | Self::UnsupportedDeductionDeclarationCannotBeUnknown
            | Self::SaltCoverageStartNotAPeriodEnd { .. }
            | Self::SaltCoverageStartOutsideTaxYear { .. }
            | Self::SaltCoverageStartBeforeEmploymentIsPayable { .. }
            | Self::OpeningBalanceFiguresOverAnEmptyCoveredSpan { .. }
            | Self::PayPeriodNotGeneratedByThePaySchedule { .. }
            | Self::PayrollRunNotFound(_)
            | Self::PayrollRunIsNotOrdinary(_)
            | Self::RemovalReasonCannotBeEmpty
            | Self::EmploymentNotAnActiveRunMember { .. }
            | Self::BasicPayCannotBeSetAsAnEarning
            | Self::PayrollRunAlreadyFinalized(_)
            | Self::PayrollRunNotCalculated(_)
            | Self::FinalizationInputMismatch { .. }
            | Self::FinalizationRulesMismatch { .. }
            | Self::FinalizationCalculationMismatch { .. }
            | Self::FinalizedPayrollNotFound(_)
            | Self::FinalizedPayrollAlreadyReversed(_)
            | Self::ReversalReasonCannotBeEmpty
            | Self::OpeningBalanceFrozenByFinalization { .. }
            | Self::PriorEmploymentFrozenByFinalization { .. }
            | Self::PayScheduleFrozenByFinalization { .. } => None,
        }
    }
}

/// At most this many differing fields are named. A refusal is read by a
/// person deciding what to fix, and a list longer than this says "recalculate
/// and look again" more usefully than an exhaustive dump does.
const MAX_NAMED_DIFFERENCES: usize = 5;

/// Longer values are cut to this many characters. A `PayrollRules` holds
/// whole PAYE band tables, and an unbounded refusal message is one nobody
/// reads.
const MAX_VALUE_CHARS: usize = 60;

/// Names the fields that differ between what finalization approved and what
/// it rebuilt, for the three refusals §5.2 raises. ADR-0010 makes this the
/// requirement it is: the refusal must name **which of the three** differed
/// *and what changed inside it*, because "a refusal that says only
/// 'something changed' is one users learn to click past".
///
/// The two values are walked as JSON rather than compared field by field in
/// Rust. That is not a shortcut: JSON is exactly what a `FinalizedPayroll`
/// freezes and what `working_payroll_calculation` stores, so every path named
/// here is a path a reader of the stored snapshot can find, and a new field on
/// any of the three types is described without this function being touched.
///
/// `None` means the value could not be serialized. That cannot happen for the
/// three types this is called with — the same serialization is an `expect`
/// wherever they are written — but `Display` must not panic, so it is reported
/// rather than unwrapped.
fn describe_differences(
    approved: Option<serde_json::Value>,
    current: Option<serde_json::Value>,
) -> String {
    let (Some(approved), Some(current)) = (approved, current) else {
        return "the differing fields could not be described".to_string();
    };

    let mut differences = Vec::new();
    collect_differences("", &approved, &current, &mut differences);

    match differences.len() {
        0 => "the differing fields could not be described".to_string(),
        n if n > MAX_NAMED_DIFFERENCES => format!(
            "{}, and further fields differ",
            differences[..MAX_NAMED_DIFFERENCES].join("; ")
        ),
        _ => differences.join("; "),
    }
}

/// Walks two JSON values in step, pushing `path: approved X, current Y` for
/// every leaf that disagrees. Recurses into objects and into arrays of equal
/// length; anything else that differs is reported at the level it differs on,
/// so a changed Earning line reads as one difference rather than as every
/// field of every line after it.
///
/// Stops one past [`MAX_NAMED_DIFFERENCES`] — enough for the caller to know
/// the list was cut, without walking a whole PAYE table to say so.
fn collect_differences(
    path: &str,
    approved: &serde_json::Value,
    current: &serde_json::Value,
    out: &mut Vec<String>,
) {
    if approved == current || out.len() > MAX_NAMED_DIFFERENCES {
        return;
    }

    match (approved, current) {
        (serde_json::Value::Object(approved), serde_json::Value::Object(current)) => {
            let null = serde_json::Value::Null;
            let keys = approved
                .keys()
                .chain(current.keys().filter(|key| !approved.contains_key(*key)));
            for key in keys {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                collect_differences(
                    &child,
                    approved.get(key).unwrap_or(&null),
                    current.get(key).unwrap_or(&null),
                    out,
                );
            }
        }
        (serde_json::Value::Array(approved), serde_json::Value::Array(current))
            if approved.len() == current.len() =>
        {
            for (index, (approved, current)) in approved.iter().zip(current).enumerate() {
                collect_differences(&format!("{path}[{index}]"), approved, current, out);
            }
        }
        _ => out.push(format!(
            "{}: approved {}, current {}",
            if path.is_empty() { "the value" } else { path },
            abbreviate(approved),
            abbreviate(current)
        )),
    }
}

/// One JSON value as compact text, cut to [`MAX_VALUE_CHARS`] characters.
/// Cut by characters rather than bytes: the cut must not land inside one.
fn abbreviate(value: &serde_json::Value) -> String {
    let text = value.to_string();
    if text.chars().count() <= MAX_VALUE_CHARS {
        return text;
    }
    let kept: String = text.chars().take(MAX_VALUE_CHARS).collect();
    format!("{kept}…")
}

impl From<PayrollError> for PayrollAppError {
    fn from(err: PayrollError) -> Self {
        Self::Payroll(err)
    }
}

impl From<sqlx::Error> for PayrollAppError {
    /// Reduced to its message rather than kept as a live [`sqlx::Error`]:
    /// that type is neither `Clone` nor comparable, and every other variant
    /// here is both, so callers can assert on a refusal by value.
    fn from(err: sqlx::Error) -> Self {
        Self::Database(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    fn refusal() -> PayrollError {
        PayrollError::PriorEmploymentUnknown
    }

    #[test]
    fn wrapping_preserves_the_refusals_message() {
        let wrapped = PayrollAppError::from(refusal());

        assert_eq!(wrapped.to_string(), refusal().to_string());
    }

    #[test]
    fn the_wrapper_never_repeats_the_refusal_in_its_source_chain() {
        let wrapped = PayrollAppError::from(refusal());

        let mut chain = Vec::new();
        let mut next: Option<&(dyn Error + 'static)> = wrapped.source();
        while let Some(err) = next {
            chain.push(err.to_string());
            next = err.source();
        }

        assert_eq!(chain, Vec::<String>::new());
    }

    #[test]
    fn a_difference_is_named_by_its_path_through_the_frozen_snapshot() {
        let approved = serde_json::json!({"employment": {"basic_pay": {"cents": 1500000}}});
        let current = serde_json::json!({"employment": {"basic_pay": {"cents": 1600000}}});

        assert_eq!(
            describe_differences(Some(approved), Some(current)),
            "employment.basic_pay.cents: approved 1500000, current 1600000"
        );
    }

    #[test]
    fn a_field_present_on_one_side_only_is_still_named() {
        let approved = serde_json::json!({"effective_from": "2026-03-01"});
        let current = serde_json::json!({});

        assert_eq!(
            describe_differences(Some(approved), Some(current)),
            "effective_from: approved \"2026-03-01\", current null"
        );
    }

    #[test]
    fn a_long_list_of_differences_is_cut_and_says_so() {
        let approved = serde_json::json!({
            "a": 1, "b": 1, "c": 1, "d": 1, "e": 1, "f": 1, "g": 1
        });
        let current = serde_json::json!({
            "a": 2, "b": 2, "c": 2, "d": 2, "e": 2, "f": 2, "g": 2
        });

        let described = describe_differences(Some(approved), Some(current));

        assert_eq!(described.matches("approved").count(), MAX_NAMED_DIFFERENCES);
        assert!(
            described.ends_with(", and further fields differ"),
            "{described}"
        );
    }

    #[test]
    fn a_long_value_is_abbreviated_rather_than_dumped() {
        let approved = serde_json::json!({"bands": "x".repeat(500)});
        let current = serde_json::json!({"bands": "y".repeat(500)});

        let described = describe_differences(Some(approved), Some(current));

        assert!(described.contains('…'), "{described}");
        assert!(described.chars().count() < 200, "{described}");
    }

    #[test]
    fn a_value_that_cannot_be_serialized_is_reported_rather_than_unwrapped() {
        assert_eq!(
            describe_differences(None, Some(serde_json::json!({}))),
            "the differing fields could not be described"
        );
    }

    #[test]
    fn the_question_mark_operator_converts_a_refusal() {
        fn use_case() -> Result<(), PayrollAppError> {
            Err(refusal())?;
            unreachable!()
        }

        assert_eq!(use_case(), Err(PayrollAppError::Payroll(refusal())));
    }
}
