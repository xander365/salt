//! `YearToDateContext`: what has happened so far this TaxYear for one
//! Employment, supplied to the calculator rather than queried by it.

use serde::{Deserialize, Serialize};

use crate::money::Money;
use crate::tax_year::TaxYear;

/// The TaxYear, taxable remuneration, PAYE withheld, and periods elapsed so
/// far for one Employment. Required, never optional (INV-013): the first
/// period of adoption passes explicit zeros, not a missing value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct YearToDateContext {
    tax_year: TaxYear,
    prior_taxable_remuneration: Money,
    prior_paye: Money,
    periods_elapsed: u32,
}

impl YearToDateContext {
    pub fn new(
        tax_year: TaxYear,
        prior_taxable_remuneration: Money,
        prior_paye: Money,
        periods_elapsed: u32,
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
        YearToDateContext::new(tax_year, Money::ZERO, Money::ZERO, 0)
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

    pub fn periods_elapsed(self) -> u32 {
        self.periods_elapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_round_trips() {
        let ytd = YearToDateContext::new(
            TaxYear::starting(2026),
            Money::from_cents(100).unwrap(),
            Money::from_cents(10).unwrap(),
            3,
        );
        let json = serde_json::to_string(&ytd).unwrap();
        assert_eq!(
            serde_json::from_str::<YearToDateContext>(&json).unwrap(),
            ytd
        );
    }
}
