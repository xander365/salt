//! Payroll calculation core for Salt.
//!
//! A pure library: no web, database, or async dependency. That absence is
//! what makes INV-009 (no I/O in calculation) structurally true rather than
//! a convention — see `docs/domain/payroll-calculation.md`.
//!
//! INV-001 (money is exact decimal, never `f32`/`f64`) is enforced by the
//! `clippy::disallowed_types` configuration in `clippy.toml`, so a float
//! entering the crate fails the build rather than review.

#![forbid(unsafe_code)]
#![warn(clippy::disallowed_types, clippy::float_arithmetic)]

mod calculation;
mod deduction;
mod earning;
mod employment;
mod money;
mod pay_period;
mod pay_schedule;
mod rules;
mod ruleset;
mod tax_year;
mod year_to_date;

pub use calculation::{
    PayeResult, PayeTrace, PayrollCalculation, PayrollError, PayrollInput, SscResult, SscTrace,
    Warning, calculate,
};
pub use deduction::{Deduction, StatutoryDeduction};
pub use earning::Earning;
pub use employment::{
    CompensationTerms, CompensationTermsError, EmployerId, EmploymentId, EmploymentSnapshot,
    PersonId, PersonReference, RawCompensationTerms,
};
pub use money::{Money, MoneyError, round_half_up};
pub use pay_period::{PayPeriod, PayPeriodError, RawPayPeriod};
pub use pay_schedule::{
    DayOfMonth, InvalidDayOfMonth, InvalidMonth, Month, PaySchedule, PeriodEndDay,
    PeriodGenerationError,
};
pub use rules::{
    BandContribution, EffectivePeriod, PayeBand, PayrollRules, PayrollRulesError,
    RawEffectivePeriod, RawPayeBand, RawPayrollRules, RawSocialSecurityRules, RoundingRule,
    RulesetId, SocialSecurityRules, SscClamp,
};
pub use ruleset::ruleset_for;
pub use tax_year::TaxYear;
pub use year_to_date::{InvalidPeriodsElapsed, PeriodsElapsed, YearToDateContext};
