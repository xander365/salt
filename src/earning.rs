//! `Earning`: one classified line of money owed for the period, and the
//! three statutory bases those lines feed.

use serde::{Deserialize, Serialize};

use crate::money::{Money, MoneyError};

/// One classified line of money owed to the employee for the period.
/// Classification, not description, decides tax treatment: there is no
/// separate `taxable: bool` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Earning {
    /// The contractual base amount for the period. The base for social
    /// security contributions.
    BasicPay(Money),
    /// An allowance that counts toward `TaxableRemuneration`.
    TaxableAllowance(Money),
}

impl Earning {
    pub fn amount(self) -> Money {
        match self {
            Earning::BasicPay(amount) => amount,
            Earning::TaxableAllowance(amount) => amount,
        }
    }
}

/// The three statutory bases earning lines feed, each accumulated on its
/// own.
///
/// This is the whole of INV-006 in code. Gross is not "the total of the
/// lines", taxable is not "gross less something", and the social security
/// base is not either of them: the three totals never share a summation,
/// and each line states which of them it feeds in one exhaustive `match`.
/// A third `Earning` kind therefore cannot be added without deciding, in
/// that one place, what it does to all three — the compiler refuses the
/// build until it is decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RemunerationBases {
    social_security: Money,
    taxable: Money,
    gross: Money,
}

impl RemunerationBases {
    /// Accumulates the bases over `lines`. Fails only on overflow: every
    /// `Money` is already non-negative, so a running total can only grow.
    pub(crate) fn accumulate<'a>(
        lines: impl IntoIterator<Item = &'a Earning>,
    ) -> Result<Self, MoneyError> {
        let mut bases = RemunerationBases {
            social_security: Money::ZERO,
            taxable: Money::ZERO,
            gross: Money::ZERO,
        };

        for line in lines {
            // The two-way table of §5.2, stated once. `BasicPay` counts
            // toward SSC, PAYE, and gross; `TaxableAllowance` toward PAYE
            // and gross alone. Each line states which bases it feeds in
            // one exhaustive match, so a future kind cannot be added
            // without deciding its effect on all three bases in this one
            // place.
            match *line {
                Earning::BasicPay(amount) => {
                    bases.social_security = bases.social_security.checked_add(amount)?;
                    bases.taxable = bases.taxable.checked_add(amount)?;
                    bases.gross = bases.gross.checked_add(amount)?;
                }
                Earning::TaxableAllowance(amount) => {
                    bases.taxable = bases.taxable.checked_add(amount)?;
                    bases.gross = bases.gross.checked_add(amount)?;
                }
            }
        }

        Ok(bases)
    }

    /// The base for social security contributions, before the floor and
    /// ceiling clamp in `PayrollRules` is applied.
    pub(crate) fn social_security(self) -> Money {
        self.social_security
    }

    /// `TaxableRemuneration` — the base cumulative PAYE is computed on.
    pub(crate) fn taxable(self) -> Money {
        self.taxable
    }

    /// `GrossRemuneration` — everything payable, taxable or not.
    pub(crate) fn gross(self) -> Money {
        self.gross
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn money(amount: rust_decimal::Decimal) -> Money {
        Money::from_decimal(amount).unwrap()
    }

    #[test]
    fn no_lines_leaves_every_base_at_zero() {
        let bases = RemunerationBases::accumulate(&[]).unwrap();

        assert_eq!(bases.social_security(), Money::ZERO);
        assert_eq!(bases.taxable(), Money::ZERO);
        assert_eq!(bases.gross(), Money::ZERO);
    }

    #[test]
    fn basic_pay_feeds_all_three_bases() {
        let bases =
            RemunerationBases::accumulate(&[Earning::BasicPay(money(dec!(15000.00)))]).unwrap();

        assert_eq!(bases.social_security(), money(dec!(15000.00)));
        assert_eq!(bases.taxable(), money(dec!(15000.00)));
        assert_eq!(bases.gross(), money(dec!(15000.00)));
    }

    #[test]
    fn a_taxable_allowance_feeds_paye_and_gross_but_not_social_security() {
        let bases =
            RemunerationBases::accumulate(&[Earning::TaxableAllowance(money(dec!(2000.00)))])
                .unwrap();

        assert_eq!(bases.social_security(), Money::ZERO);
        assert_eq!(bases.taxable(), money(dec!(2000.00)));
        assert_eq!(bases.gross(), money(dec!(2000.00)));
    }

    #[test]
    fn social_security_differs_from_taxable_and_gross_when_an_allowance_is_present() {
        let bases = RemunerationBases::accumulate(&[
            Earning::BasicPay(money(dec!(15000.00))),
            Earning::TaxableAllowance(money(dec!(2000.00))),
        ])
        .unwrap();

        assert_eq!(bases.social_security(), money(dec!(15000.00)));
        assert_eq!(bases.taxable(), money(dec!(17000.00)));
        assert_eq!(bases.gross(), money(dec!(17000.00)));
    }

    #[test]
    fn repeated_lines_of_one_kind_each_count_once() {
        let bases = RemunerationBases::accumulate(&[
            Earning::TaxableAllowance(money(dec!(250.00))),
            Earning::TaxableAllowance(money(dec!(250.00))),
        ])
        .unwrap();

        assert_eq!(bases.taxable(), money(dec!(500.00)));
        assert_eq!(bases.gross(), money(dec!(500.00)));
    }

    #[test]
    fn an_overflowing_total_is_reported_rather_than_wrapped() {
        let huge = Money::from_cents(i64::MAX).unwrap();
        let lines = [Earning::TaxableAllowance(huge), Earning::BasicPay(huge)];

        assert_eq!(
            RemunerationBases::accumulate(&lines),
            Err(MoneyError::Overflow)
        );
    }

    #[test]
    fn amount_is_carried_by_every_kind() {
        assert_eq!(
            Earning::BasicPay(money(dec!(1.00))).amount(),
            money(dec!(1.00))
        );
        assert_eq!(
            Earning::TaxableAllowance(money(dec!(2.00))).amount(),
            money(dec!(2.00))
        );
    }

    #[test]
    fn deserialize_round_trips() {
        let line = Earning::TaxableAllowance(money(dec!(2000.00)));
        let json = serde_json::to_string(&line).unwrap();

        assert_eq!(serde_json::from_str::<Earning>(&json).unwrap(), line);
    }
}
