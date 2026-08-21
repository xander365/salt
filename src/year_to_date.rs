//! `YearToDateContext`: what has happened so far this TaxYear for one
//! Employment, supplied to the calculator rather than queried by it.

use serde::{Deserialize, Serialize};

use crate::money::Money;
use crate::tax_year::TaxYear;

/// How many periods of the TaxYear are already complete, 0 to 11.
///
/// Keying the ruleset and the TaxYear on the PayPeriod end date guarantees
/// exactly twelve periods per TaxYear (ADR-0005), and cumulative PAYE
/// depends on that: the annual band thresholds are scaled to
/// `elapsed + 1`/12. A thirteenth period would scale them past the full
/// annual table and under-tax the employee, so 12 and above are not
/// representable rather than merely wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct PeriodsElapsed(u8);

/// Why a `PeriodsElapsed` could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPeriodsElapsed(pub u8);

impl std::fmt::Display for InvalidPeriodsElapsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} is not a count of elapsed periods in 0..=11 — a TaxYear has exactly twelve periods",
            self.0
        )
    }
}

impl std::error::Error for InvalidPeriodsElapsed {}

impl PeriodsElapsed {
    /// No period of this TaxYear is complete yet.
    pub const NONE: PeriodsElapsed = PeriodsElapsed(0);

    pub fn new(elapsed: u8) -> Result<Self, InvalidPeriodsElapsed> {
        if elapsed <= 11 {
            Ok(PeriodsElapsed(elapsed))
        } else {
            Err(InvalidPeriodsElapsed(elapsed))
        }
    }

    pub fn get(self) -> u8 {
        self.0
    }

    /// Which period of the TaxYear is being calculated, counting the
    /// current one: 1 for the first, 12 for the last. This is the figure
    /// the PAYE band thresholds are scaled by.
    pub(crate) fn period_number(self) -> u32 {
        self.0 as u32 + 1
    }
}

impl TryFrom<u8> for PeriodsElapsed {
    type Error = InvalidPeriodsElapsed;

    fn try_from(elapsed: u8) -> Result<Self, InvalidPeriodsElapsed> {
        PeriodsElapsed::new(elapsed)
    }
}

impl From<PeriodsElapsed> for u8 {
    fn from(elapsed: PeriodsElapsed) -> u8 {
        elapsed.0
    }
}

/// The TaxYear, taxable remuneration, PAYE withheld, and periods elapsed so
/// far for one Employment. Required, never optional (INV-013): the first
/// period of adoption passes explicit zeros, not a missing value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct YearToDateContext {
    tax_year: TaxYear,
    prior_taxable_remuneration: Money,
    prior_paye: Money,
    periods_elapsed: PeriodsElapsed,
}

impl YearToDateContext {
    pub fn new(
        tax_year: TaxYear,
        prior_taxable_remuneration: Money,
        prior_paye: Money,
        periods_elapsed: PeriodsElapsed,
    ) -> Self {
        YearToDateContext {
            tax_year,
            prior_taxable_remuneration,
            prior_paye,
            periods_elapsed,
        }
    }

    /// The first period of a tax year: no prior remuneration, no prior
    /// PAYE, no periods elapsed.
    pub fn first_period(tax_year: TaxYear) -> Self {
        YearToDateContext::new(tax_year, Money::ZERO, Money::ZERO, PeriodsElapsed::NONE)
    }

    pub fn tax_year(self) -> TaxYear {
        self.tax_year
    }

    pub fn prior_taxable_remuneration(self) -> Money {
        self.prior_taxable_remuneration
    }

    pub fn prior_paye(self) -> Money {
        self.prior_paye
    }

    pub fn periods_elapsed(self) -> PeriodsElapsed {
        self.periods_elapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods_elapsed_accepts_0_to_11() {
        assert_eq!(PeriodsElapsed::new(0).unwrap().get(), 0);
        assert_eq!(PeriodsElapsed::new(11).unwrap().get(), 11);
    }

    #[test]
    fn periods_elapsed_rejects_a_thirteenth_period() {
        assert_eq!(PeriodsElapsed::new(12), Err(InvalidPeriodsElapsed(12)));
        assert_eq!(PeriodsElapsed::new(255), Err(InvalidPeriodsElapsed(255)));
    }

    #[test]
    fn period_number_counts_the_current_period() {
        assert_eq!(PeriodsElapsed::NONE.period_number(), 1);
        assert_eq!(PeriodsElapsed::new(11).unwrap().period_number(), 12);
    }

    #[test]
    fn periods_elapsed_deserialize_rejects_out_of_range() {
        assert!(serde_json::from_str::<PeriodsElapsed>("12").is_err());
    }

    #[test]
    fn first_period_is_all_zeros() {
        let ytd = YearToDateContext::first_period(TaxYear::starting(2026));
        assert_eq!(ytd.prior_taxable_remuneration(), Money::ZERO);
        assert_eq!(ytd.prior_paye(), Money::ZERO);
        assert_eq!(ytd.periods_elapsed(), PeriodsElapsed::NONE);
    }

    #[test]
    fn deserialize_round_trips() {
        let ytd = YearToDateContext::new(
            TaxYear::starting(2026),
            Money::from_cents(100).unwrap(),
            Money::from_cents(10).unwrap(),
            PeriodsElapsed::new(3).unwrap(),
        );
        let json = serde_json::to_string(&ytd).unwrap();
        assert_eq!(
            serde_json::from_str::<YearToDateContext>(&json).unwrap(),
            ytd
        );
    }
}
