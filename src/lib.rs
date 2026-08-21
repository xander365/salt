//! Payroll calculation core for Salt.
//!
//! A pure library: no web, database, or async dependency. That absence is
//! what makes INV-009 (no I/O in calculation) structurally true rather than
//! a convention — see `docs/domain/payroll-calculation.md`.

mod calculation;
mod deduction;
mod earning;
mod employment;
mod money;
mod pay_period;
mod pay_schedule;
mod rules;
mod year_to_date;

pub use calculation::{
    PayeResult, PayeTrace, PayrollCalculation, PayrollError, PayrollInput, SscResult, SscTrace,
    calculate,
};
pub use deduction::{Deduction, StatutoryDeduction};
pub use earning::Earning;
pub use employment::{CompensationTerms, EmploymentSnapshot};
pub use money::{Money, MoneyError, round_half_up};
pub use pay_period::{PayPeriod, PayPeriodError};
pub use pay_schedule::{DayOfMonth, InvalidDayOfMonth, PaySchedule, PeriodEndDay};
pub use rules::{PayeBand, PayrollRules, RoundingRule, SocialSecurityRules};
pub use year_to_date::YearToDateContext;
