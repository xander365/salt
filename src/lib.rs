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
mod tax_year;
mod year_to_date;

pub use calculation::{
    PayeResult, PayeTrace, PayrollCalculation, PayrollError, PayrollInput, SscResult, SscTrace,
    Warning, calculate,
};
pub use deduction::{Deduction, StatutoryDeduction};
pub use earning::Earning;
pub use employment::{CompensationTerms, EmploymentId, EmploymentSnapshot};
pub use money::{Money, MoneyError, round_half_up};
pub use pay_period::{PayPeriod, PayPeriodError, RawPayPeriod};
pub use pay_schedule::{
    DayOfMonth, InvalidDayOfMonth, InvalidMonth, Month, PaySchedule, PeriodEndDay,
};
pub use rules::{
    BandContribution, PayeBand, PayrollRules, PayrollRulesError, RawPayeBand, RawPayrollRules,
    RawSocialSecurityRules, RoundingRule, SocialSecurityRules, SscClamp,
};
pub use tax_year::TaxYear;
pub use year_to_date::YearToDateContext;
