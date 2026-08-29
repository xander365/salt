use chrono::NaiveDate;
use payroll::{EmployerId, EmploymentId, PayrollError, TaxYear};

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
            | Self::OpeningBalanceFiguresOverAnEmptyCoveredSpan { .. } => None,
        }
    }
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
    fn the_question_mark_operator_converts_a_refusal() {
        fn use_case() -> Result<(), PayrollAppError> {
            Err(refusal())?;
            unreachable!()
        }

        assert_eq!(use_case(), Err(PayrollAppError::Payroll(refusal())));
    }
}
