//! `TaxYear`: the Namibian tax year, 1 March to end of February.

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

/// The Namibian tax year running from 1 March of `starting_year` to the
/// last day of February the following year. Identified by the calendar
/// year it starts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TaxYear(i32);

impl TaxYear {
    pub fn starting(starting_year: i32) -> Self {
        TaxYear(starting_year)
    }

    pub fn starting_year(self) -> i32 {
        self.0
    }

    /// The `TaxYear` a `PayPeriod` end date falls in (ADR-0005). January
    /// and February belong to the tax year that started the previous
    /// March, so a period of 26 February to 25 March falls wholly in the
    /// tax year that starts that March, even though it starts in the one
    /// before.
    pub fn for_period_end(date: NaiveDate) -> Self {
        if date.month() >= 3 {
            TaxYear::starting(date.year())
        } else {
            TaxYear::starting(date.year() - 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pay_schedule::{DayOfMonth, Month, PaySchedule, PeriodEndDay};

    #[test]
    fn starting_year_round_trips() {
        assert_eq!(TaxYear::starting(2026).starting_year(), 2026);
    }

    #[test]
    fn deserialize_round_trips() {
        let tax_year = TaxYear::starting(2026);
        let json = serde_json::to_string(&tax_year).unwrap();
        assert_eq!(serde_json::from_str::<TaxYear>(&json).unwrap(), tax_year);
    }

    #[test]
    fn a_date_from_march_onward_belongs_to_the_tax_year_starting_that_year() {
        let date = NaiveDate::from_ymd_opt(2026, 3, 1).unwrap();
        assert_eq!(TaxYear::for_period_end(date), TaxYear::starting(2026));

        let date = NaiveDate::from_ymd_opt(2026, 12, 31).unwrap();
        assert_eq!(TaxYear::for_period_end(date), TaxYear::starting(2026));
    }

    #[test]
    fn a_date_in_january_or_february_belongs_to_the_previous_starting_year() {
        let date = NaiveDate::from_ymd_opt(2027, 1, 1).unwrap();
        assert_eq!(TaxYear::for_period_end(date), TaxYear::starting(2026));

        let date = NaiveDate::from_ymd_opt(2027, 2, 28).unwrap();
        assert_eq!(TaxYear::for_period_end(date), TaxYear::starting(2026));
    }

    // A 26 Feb - 25 Mar period straddles the tax year boundary by
    // calendar date, but ADR-0005 keys the TaxYear on the period end
    // alone, so the whole period falls in the new tax year.
    #[test]
    fn a_26_feb_to_25_mar_period_falls_wholly_in_the_new_tax_year() {
        let start = NaiveDate::from_ymd_opt(2026, 2, 26).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 3, 25).unwrap();

        assert_eq!(TaxYear::for_period_end(start), TaxYear::starting(2025));
        assert_eq!(TaxYear::for_period_end(end), TaxYear::starting(2026));
    }

    // Cumulative PAYE (ADR-0001) depends on every employer getting exactly
    // twelve periods per tax year — asserted directly here rather than
    // trusted as incidental.
    //
    // Three tax years of periods are generated and counted, so the twelve
    // is the answer to "how many landed in this tax year", not a
    // restatement of how many were asked for. The neighbouring years are
    // included so a period leaking across either boundary would show up as
    // a thirteenth or an eleventh.
    #[test]
    fn a_26th_to_25th_schedule_yields_exactly_twelve_periods_in_one_tax_year() {
        assert_periods_per_tax_year(PeriodEndDay::Day(DayOfMonth::new(25).unwrap()));
    }

    // The same property must hold for a calendar-month employer, which is
    // a `PeriodEndDay` setting rather than a separate code path (ADR-0005).
    #[test]
    fn a_calendar_month_schedule_also_yields_exactly_twelve_periods_in_one_tax_year() {
        assert_periods_per_tax_year(PeriodEndDay::LastDayOfMonth);
    }

    fn assert_periods_per_tax_year(end_day: PeriodEndDay) {
        let schedule = PaySchedule::new(end_day);
        let periods = schedule
            .generate_periods(2025, Month::new(3).unwrap(), 36)
            .unwrap();

        for starting_year in [2025, 2026, 2027] {
            let tax_year = TaxYear::starting(starting_year);
            let count = periods
                .iter()
                .filter(|period| TaxYear::for_period_end(period.end()) == tax_year)
                .count();
            assert_eq!(count, 12, "tax year starting {starting_year}");
        }

        // Every generated period is accounted for by those three years, so
        // none escaped the count above into a fourth.
        assert_eq!(periods.len(), 36);
    }

    // Consecutive periods are contiguous and non-overlapping, so the
    // twelve above tile the tax year rather than merely numbering twelve.
    #[test]
    fn consecutive_periods_leave_no_gap_and_no_overlap() {
        let schedule = PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap()));
        let periods = schedule
            .generate_periods(2026, Month::new(3).unwrap(), 12)
            .unwrap();

        for pair in periods.windows(2) {
            assert_eq!(pair[0].end().succ_opt(), Some(pair[1].start()));
        }
    }
}
