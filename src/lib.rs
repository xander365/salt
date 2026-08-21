//! Payroll calculation core for Salt.
//!
//! A pure library: no web, database, or async dependency. That absence is
//! what makes INV-009 (no I/O in calculation) structurally true rather than
//! a convention — see `docs/domain/payroll-calculation.md`.

mod money;
mod pay_period;
mod pay_schedule;

pub use money::{Money, MoneyError, round_half_up};
pub use pay_period::{PayPeriod, PayPeriodError};
pub use pay_schedule::{DayOfMonth, InvalidDayOfMonth, PaySchedule, PeriodEndDay};
