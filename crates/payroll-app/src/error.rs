use payroll::{EmploymentId, PayrollError};

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
    /// No Employment exists with this id.
    EmploymentNotFound(EmploymentId),
    /// No `CompensationTerms` row is in force for this Employment as of the
    /// requested date.
    NoCompensationTermsInForce(EmploymentId),
}

impl std::fmt::Display for PayrollAppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Payroll(err) => write!(f, "{err}"),
            Self::Database(message) => write!(f, "database error: {message}"),
            Self::EmploymentNotFound(id) => write!(f, "no Employment exists with id {id}"),
            Self::NoCompensationTermsInForce(id) => {
                write!(f, "no CompensationTerms are in force for Employment {id}")
            }
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
            | Self::EmploymentNotFound(_)
            | Self::NoCompensationTermsInForce(_) => None,
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
