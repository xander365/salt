//! `Earning`: one classified line of money owed for the period.

use crate::money::Money;

/// One classified line of money owed to the employee for the period.
/// Classification, not description, decides tax treatment: there is no
/// separate `taxable: bool` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Earning {
    /// The contractual base amount for the period. The base for social
    /// security contributions.
    BasicPay(Money),
    /// An allowance that counts toward `TaxableRemuneration`.
    TaxableAllowance(Money),
    /// An allowance that counts toward `GrossRemuneration` only.
    NonTaxableAllowance(Money),
}

impl Earning {
    pub fn amount(self) -> Money {
        match self {
            Earning::BasicPay(amount) => amount,
            Earning::TaxableAllowance(amount) => amount,
            Earning::NonTaxableAllowance(amount) => amount,
        }
    }

    pub(crate) fn is_taxable(self) -> bool {
        matches!(self, Earning::BasicPay(_) | Earning::TaxableAllowance(_))
    }
}
