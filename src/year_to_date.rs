//! `YearToDateContext`: what has happened so far this TaxYear for one
//! Employment, supplied to the calculator rather than queried by it.

use crate::money::Money;

/// The taxable remuneration, PAYE withheld, and periods elapsed so far in
/// the TaxYear for one Employment. Required, never optional (INV-013): the
/// first period of adoption passes explicit zeros, not a missing value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct YearToDateContext {
    prior_taxable_remuneration: Money,
    prior_paye: Money,
    periods_elapsed: u32,
}

impl YearToDateContext {
    pub fn new(prior_taxable_remuneration: Money, prior_paye: Money, periods_elapsed: u32) -> Self {
        YearToDateContext {
            prior_taxable_remuneration,
            prior_paye,
            periods_elapsed,
        }
    }

    /// The first period of a tax year: no prior remuneration, no prior
    /// PAYE, no periods elapsed.
    pub fn first_period() -> Self {
        YearToDateContext::new(Money::ZERO, Money::ZERO, 0)
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
