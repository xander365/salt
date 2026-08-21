//! `PayPeriod`: the span of work being paid for.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

/// The span of work being paid for. Carries no pay date — see
/// `docs/domain/payroll-calculation.md` §4.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "RawPayPeriod", into = "RawPayPeriod")]
pub struct PayPeriod {
    start: NaiveDate,
    end: NaiveDate,
}

/// The wire shape of a `PayPeriod`, validated on the way in by `PayPeriod`'s
/// `TryFrom` impl so deserialization cannot bypass the end-after-start
/// invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawPayPeriod {
    pub start: NaiveDate,
    pub end: NaiveDate,
}

/// Why a `PayPeriod` could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayPeriodError {
    /// The end date was before the start date.
    EndBeforeStart,
}

impl std::fmt::Display for PayPeriodError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PayPeriodError::EndBeforeStart => write!(f, "end date is before start date"),
        }
    }
}

impl std::error::Error for PayPeriodError {}

impl PayPeriod {
    pub fn new(start: NaiveDate, end: NaiveDate) -> Result<Self, PayPeriodError> {
        if end < start {
            Err(PayPeriodError::EndBeforeStart)
        } else {
            Ok(PayPeriod { start, end })
        }
    }

    pub fn start(self) -> NaiveDate {
        self.start
    }

    pub fn end(self) -> NaiveDate {
        self.end
    }
}

impl TryFrom<RawPayPeriod> for PayPeriod {
    type Error = PayPeriodError;

    fn try_from(raw: RawPayPeriod) -> Result<Self, PayPeriodError> {
        PayPeriod::new(raw.start, raw.end)
    }
}

impl From<PayPeriod> for RawPayPeriod {
    fn from(period: PayPeriod) -> RawPayPeriod {
        RawPayPeriod {
            start: period.start,
            end: period.end,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    #[test]
    fn accepts_end_after_start() {
        let period = PayPeriod::new(date(2026, 1, 26), date(2026, 2, 25)).unwrap();
        assert_eq!(period.start(), date(2026, 1, 26));
        assert_eq!(period.end(), date(2026, 2, 25));
    }

    #[test]
    fn accepts_a_single_day_period() {
        let period = PayPeriod::new(date(2026, 1, 1), date(2026, 1, 1)).unwrap();
        assert_eq!(period.start(), period.end());
    }

    #[test]
    fn rejects_end_before_start() {
        let result = PayPeriod::new(date(2026, 2, 25), date(2026, 1, 26));
        assert_eq!(result, Err(PayPeriodError::EndBeforeStart));
    }

    #[test]
    fn deserialize_round_trips() {
        let period = PayPeriod::new(date(2026, 1, 26), date(2026, 2, 25)).unwrap();
        let json = serde_json::to_string(&period).unwrap();
        assert_eq!(serde_json::from_str::<PayPeriod>(&json).unwrap(), period);
    }

    #[test]
    fn deserialize_rejects_end_before_start() {
        let json = r#"{"start":"2026-02-25","end":"2026-01-26"}"#;
        assert!(serde_json::from_str::<PayPeriod>(json).is_err());
    }
}
