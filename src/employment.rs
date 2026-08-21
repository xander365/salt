//! The Employment facts the calculator needs for one PayPeriod.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::money::Money;
use crate::pay_period::PayPeriod;

/// Identifies which Employment a `PayrollCalculation` was produced for.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EmploymentId(String);

impl EmploymentId {
    pub fn new(id: impl Into<String>) -> Self {
        EmploymentId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What the Employment agrees to pay, over an effective period. v1
/// supports monthly `BasicPay` only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

    pub fn basic_pay(&self) -> Money {
        self.basic_pay
    }

    /// Whether these terms are in force for every day of `period`.
    pub(crate) fn covers(&self, period: PayPeriod) -> bool {
        self.effective_from <= period.start()
            && self
                .effective_until
                .is_none_or(|until| until >= period.end())
    }
}

/// The Employment facts relevant to calculating one PayPeriod: independent
/// of Person and Employer, which the calculator never needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmploymentSnapshot {
    employment_id: EmploymentId,
    start_date: NaiveDate,
    end_date: Option<NaiveDate>,
    compensation_terms: CompensationTerms,
}

impl EmploymentSnapshot {
    pub fn new(
        employment_id: EmploymentId,
        start_date: NaiveDate,
        end_date: Option<NaiveDate>,
        compensation_terms: CompensationTerms,
    ) -> Self {
        EmploymentSnapshot {
            employment_id,
            start_date,
            end_date,
            compensation_terms,
        }
    }

    pub fn employment_id(&self) -> &EmploymentId {
        &self.employment_id
    }

    pub fn start_date(&self) -> NaiveDate {
        self.start_date
    }

    pub fn end_date(&self) -> Option<NaiveDate> {
        self.end_date
    }

    pub fn compensation_terms(&self) -> CompensationTerms {
        self.compensation_terms
    }

    /// Whether `start_date` and `end_date` are internally coherent.
    pub(crate) fn has_coherent_dates(&self) -> bool {
        self.end_date.is_none_or(|end| end >= self.start_date)
    }

    /// Whether this Employment covers every day of `period` — i.e. it is
    /// not a joiner starting mid-period or a leaver ending mid-period.
    /// Proration for partial periods is a later ticket; until then such a
    /// period is refused rather than paid in full or guessed at.
    pub(crate) fn covers_full_period(&self, period: PayPeriod) -> bool {
        self.start_date <= period.start() && self.end_date.is_none_or(|end| end >= period.end())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_round_trips() {
        let terms = CompensationTerms::new(
            NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
            None,
            Money::from_cents(500000).unwrap(),
        );
        let snapshot = EmploymentSnapshot::new(
            EmploymentId::new("emp-1"),
            NaiveDate::from_ymd_opt(2025, 1, 1).unwrap(),
            None,
            terms,
        );
        let json = serde_json::to_string(&snapshot).unwrap();
        assert_eq!(
            serde_json::from_str::<EmploymentSnapshot>(&json).unwrap(),
            snapshot
        );
    }
}
