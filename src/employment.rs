//! The Employment facts the calculator needs for one PayPeriod.

use chrono::NaiveDate;

use crate::money::Money;

/// What the Employment agrees to pay, over an effective period. v1
/// supports monthly `BasicPay` only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompensationTerms {
    effective_from: NaiveDate,
    effective_until: Option<NaiveDate>,
    basic_pay: Money,
}

impl CompensationTerms {
    pub fn new(
        effective_from: NaiveDate,
        effective_until: Option<NaiveDate>,
        basic_pay: Money,
    ) -> Self {
        CompensationTerms {
            effective_from,
            effective_until,
            basic_pay,
        }
    }

    pub fn basic_pay(self) -> Money {
        self.basic_pay
    }

    /// Whether these terms are in force for every day of `period`.
    pub(crate) fn covers(self, period: crate::pay_period::PayPeriod) -> bool {
        self.effective_from <= period.start()
            && self
                .effective_until
                .is_none_or(|until| until >= period.end())
    }
}

/// The Employment facts relevant to calculating one PayPeriod: independent
/// of Person and Employer, which the calculator never needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmploymentSnapshot {
    start_date: NaiveDate,
    end_date: Option<NaiveDate>,
    compensation_terms: CompensationTerms,
}

impl EmploymentSnapshot {
    pub fn new(
        start_date: NaiveDate,
        end_date: Option<NaiveDate>,
        compensation_terms: CompensationTerms,
    ) -> Self {
        EmploymentSnapshot {
            start_date,
            end_date,
            compensation_terms,
        }
    }

    pub fn start_date(self) -> NaiveDate {
        self.start_date
    }

    pub fn end_date(self) -> Option<NaiveDate> {
        self.end_date
    }

    pub fn compensation_terms(self) -> CompensationTerms {
        self.compensation_terms
    }

    /// Whether `start_date` and `end_date` are internally coherent.
    pub(crate) fn has_coherent_dates(self) -> bool {
        self.end_date.is_none_or(|end| end >= self.start_date)
    }
}
