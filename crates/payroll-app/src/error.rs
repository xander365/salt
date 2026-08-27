use payroll::PayrollError;

/// `payroll-app`'s own error type. It wraps [`PayrollError`] rather than
/// re-exporting it, because "PostgreSQL unavailable" and "run already
/// finalized" are different categories and never share an enum (ADR-0009).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayrollAppError {
    Payroll(PayrollError),
}

impl std::fmt::Display for PayrollAppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Payroll(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for PayrollAppError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Payroll(err) => Some(err),
        }
    }
}

impl From<PayrollError> for PayrollAppError {
    fn from(err: PayrollError) -> Self {
        Self::Payroll(err)
    }
}
