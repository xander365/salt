//! `PayPeriod`: the span of work being paid for.

use chrono::NaiveDate;

/// The span of work being paid for. Carries no pay date — see
/// `docs/domain/payroll-calculation.md` §4.5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PayPeriod {
    start: NaiveDate,
    end: NaiveDate,
}

/// Why a `PayPeriod` could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayPeriodError {
    /// The end date was before the start date.
    EndBeforeStart,
}

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
}
