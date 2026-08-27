//! The stateful application layer for Salt, built on the pure `payroll`
//! crate (ADR-0009). `payroll-app` depends on `payroll`; the reverse
//! dependency does not exist.

mod error;

pub use error::PayrollAppError;
