//! `PaySchedule`: how an Employer's pay cycle works, and `PayPeriod`
//! generation from it. See ADR-0005.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::pay_period::PayPeriod;

/// A validated day-of-month, 1 to 28 inclusive. Constructing this is the
/// only way to obtain a value that can go into `PeriodEndDay::Day` — the
/// field is private, so 29, 30, and 31 are not representable, not merely
/// rejected at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct DayOfMonth(u8);

/// Why a `DayOfMonth` could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidDayOfMonth(pub u8);

impl std::fmt::Display for InvalidDayOfMonth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} is not a day of month in 1..=28", self.0)
    }
}

impl std::error::Error for InvalidDayOfMonth {}

impl DayOfMonth {
    pub fn new(day: u8) -> Result<Self, InvalidDayOfMonth> {
        if (1..=28).contains(&day) {
            Ok(DayOfMonth(day))
        } else {
            Err(InvalidDayOfMonth(day))
        }
    }

    pub fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for DayOfMonth {
    type Error = InvalidDayOfMonth;

    fn try_from(day: u8) -> Result<Self, InvalidDayOfMonth> {
        DayOfMonth::new(day)
    }
}

impl From<DayOfMonth> for u8 {
    fn from(day: DayOfMonth) -> u8 {
        day.0
    }
}

/// A validated calendar month, 1 (January) to 12 (December). Unlike
/// `DayOfMonth`, this is not a payroll concept of its own — it exists so
/// `PaySchedule::generate_periods` cannot be called with an out-of-range
/// month and reach an internal panic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Month(u8);

/// Why a `Month` could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidMonth(pub u8);

impl std::fmt::Display for InvalidMonth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} is not a month in 1..=12", self.0)
    }
}

impl std::error::Error for InvalidMonth {}

impl Month {
    pub fn new(month: u8) -> Result<Self, InvalidMonth> {
        if (1..=12).contains(&month) {
            Ok(Month(month))
        } else {
            Err(InvalidMonth(month))
        }
    }

    pub fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for Month {
    type Error = InvalidMonth;

    fn try_from(month: u8) -> Result<Self, InvalidMonth> {
        Month::new(month)
    }
}

impl From<Month> for u8 {
    fn from(month: Month) -> u8 {
        month.0
    }
}

/// The day of the month a pay period ends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PeriodEndDay {
    /// A fixed day, 1 to 28. E.g. a 26th-to-25th cycle ends on `Day(25)`.
    Day(DayOfMonth),
    /// The last day of the month, whatever it is (28, 29, 30, or 31). A
    /// distinct variant, not day 31 with clamping (ADR-0005) — clamping
    /// would turn a 31-day-ending schedule into a 3-day February period.
    LastDayOfMonth,
}

/// How an Employer's pay cycle works. v1 is monthly only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaySchedule {
    period_end_day: PeriodEndDay,
}

impl PaySchedule {
    pub fn new(period_end_day: PeriodEndDay) -> Self {
        PaySchedule { period_end_day }
    }

    pub fn period_end_day(self) -> PeriodEndDay {
        self.period_end_day
    }

    /// The end date of the period ending in the given year and month.
    fn end_date(self, year: i32, month: u32) -> NaiveDate {
        match self.period_end_day {
            PeriodEndDay::Day(day) => NaiveDate::from_ymd_opt(year, month, day.get() as u32)
                .expect("day 1-28 is valid in every month"),
            PeriodEndDay::LastDayOfMonth => last_day_of_month(year, month),
        }
    }

    /// Generates `count` consecutive `PayPeriod`s, keyed on the period end
    /// day, starting with the period whose end falls in `from_year`/
    /// `from_month`. Each period's start is the day after the previous
    /// period's end — including the one before `from_month` — so the
    /// sequence has no gaps and no overlaps regardless of month length,
    /// and `Day` and `LastDayOfMonth` schedules run through this same code
    /// path.
    pub fn generate_periods(self, from_year: i32, from_month: Month, count: u32) -> Vec<PayPeriod> {
        let (mut year, mut month) = (from_year, from_month.get() as u32);
        let (prev_year, prev_month) = previous_month(from_year, from_month.get() as u32);
        let mut previous_end = self.end_date(prev_year, prev_month);

        let mut periods = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let end = self.end_date(year, month);
            let start = previous_end
                .succ_opt()
                .expect("payroll dates stay well within chrono's range");
            periods.push(PayPeriod::new(start, end).expect("previous_end < end by construction"));

            previous_end = end;
            (year, month) = next_month(year, month);
        }
        periods
    }
}

fn next_month(year: i32, month: u32) -> (i32, u32) {
    if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    }
}

fn previous_month(year: i32, month: u32) -> (i32, u32) {
    if month == 1 {
        (year - 1, 12)
    } else {
        (year, month - 1)
    }
}

fn last_day_of_month(year: i32, month: u32) -> NaiveDate {
    let (next_year, next_month) = next_month(year, month);
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .expect("month rolled over to a valid year/month")
        .pred_opt()
        .expect("the first of a month always has a preceding day")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn day(n: u8) -> DayOfMonth {
        DayOfMonth::new(n).unwrap()
    }

    fn month(n: u8) -> Month {
        Month::new(n).unwrap()
    }

    #[test]
    fn day_of_month_accepts_1_to_28() {
        assert!(DayOfMonth::new(1).is_ok());
        assert!(DayOfMonth::new(28).is_ok());
    }

    #[test]
    fn day_of_month_rejects_29_30_31_and_0() {
        assert_eq!(DayOfMonth::new(29), Err(InvalidDayOfMonth(29)));
        assert_eq!(DayOfMonth::new(30), Err(InvalidDayOfMonth(30)));
        assert_eq!(DayOfMonth::new(31), Err(InvalidDayOfMonth(31)));
        assert_eq!(DayOfMonth::new(0), Err(InvalidDayOfMonth(0)));
    }

    #[test]
    fn month_accepts_1_to_12() {
        assert!(Month::new(1).is_ok());
        assert!(Month::new(12).is_ok());
    }

    #[test]
    fn month_rejects_0_and_13() {
        assert_eq!(Month::new(0), Err(InvalidMonth(0)));
        assert_eq!(Month::new(13), Err(InvalidMonth(13)));
    }

    #[test]
    fn a_26th_to_25th_schedule_across_a_full_year_has_no_gaps_or_overlaps() {
        let schedule = PaySchedule::new(PeriodEndDay::Day(day(25)));
        let periods = schedule.generate_periods(2026, month(1), 12);

        assert_eq!(periods.len(), 12);
        assert_eq!(periods[0].start(), date(2025, 12, 26));
        assert_eq!(periods[0].end(), date(2026, 1, 25));
        assert_eq!(periods[11].start(), date(2026, 11, 26));
        assert_eq!(periods[11].end(), date(2026, 12, 25));

        for period in &periods {
            let length = (period.end() - period.start()).num_days() + 1;
            assert!(
                (28..=31).contains(&length),
                "period {:?}-{:?} has length {length}",
                period.start(),
                period.end()
            );
        }

        for pair in periods.windows(2) {
            assert_eq!(
                pair[1].start(),
                pair[0].end().succ_opt().unwrap(),
                "gap or overlap between {:?} and {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn a_26th_to_25th_schedule_survives_a_leap_year() {
        let schedule = PaySchedule::new(PeriodEndDay::Day(day(25)));
        // 2028 is a leap year: February has 29 days.
        let periods = schedule.generate_periods(2028, month(1), 12);

        let feb_period = periods
            .iter()
            .find(|p| p.end() == date(2028, 2, 25))
            .unwrap();
        assert_eq!(feb_period.start(), date(2028, 1, 26));

        let mar_period = periods
            .iter()
            .find(|p| p.end() == date(2028, 3, 25))
            .unwrap();
        assert_eq!(mar_period.start(), date(2028, 2, 26));
        // Feb 26-29 (4 days) + Mar 1-25 (25 days) = 29, one more than a
        // non-leap year would give.
        let length = (mar_period.end() - mar_period.start()).num_days() + 1;
        assert_eq!(length, 29);

        for pair in periods.windows(2) {
            assert_eq!(pair[1].start(), pair[0].end().succ_opt().unwrap());
        }
    }

    #[test]
    fn a_calendar_month_schedule_runs_through_the_same_code_path() {
        let schedule = PaySchedule::new(PeriodEndDay::LastDayOfMonth);
        let periods = schedule.generate_periods(2026, month(1), 12);

        assert_eq!(periods.len(), 12);
        assert_eq!(periods[0].start(), date(2026, 1, 1));
        assert_eq!(periods[0].end(), date(2026, 1, 31));
        assert_eq!(periods[1].start(), date(2026, 2, 1));
        assert_eq!(periods[1].end(), date(2026, 2, 28));
        assert_eq!(periods[11].start(), date(2026, 12, 1));
        assert_eq!(periods[11].end(), date(2026, 12, 31));

        for pair in periods.windows(2) {
            assert_eq!(pair[1].start(), pair[0].end().succ_opt().unwrap());
        }
    }

    #[test]
    fn a_calendar_month_schedule_survives_a_leap_year_february() {
        let schedule = PaySchedule::new(PeriodEndDay::LastDayOfMonth);
        let periods = schedule.generate_periods(2028, month(1), 12);

        let feb_period = periods
            .iter()
            .find(|p| p.start() == date(2028, 2, 1))
            .unwrap();
        assert_eq!(feb_period.end(), date(2028, 2, 29));
    }

    #[test]
    fn generation_crosses_a_year_boundary_without_a_gap() {
        let schedule = PaySchedule::new(PeriodEndDay::Day(day(25)));
        let periods = schedule.generate_periods(2026, month(12), 3);

        assert_eq!(periods[0].end(), date(2026, 12, 25));
        assert_eq!(periods[1].start(), date(2026, 12, 26));
        assert_eq!(periods[1].end(), date(2027, 1, 25));
        assert_eq!(periods[2].start(), date(2027, 1, 26));
        assert_eq!(periods[2].end(), date(2027, 2, 25));
    }

    #[test]
    fn day_of_month_deserialize_rejects_out_of_range() {
        assert!(serde_json::from_str::<DayOfMonth>("29").is_err());
    }

    #[test]
    fn month_deserialize_rejects_out_of_range() {
        assert!(serde_json::from_str::<Month>("13").is_err());
    }
}
