//! `PayrollRules`: the statutory and agreed calculation rules in force for
//! an effective period, as typed Rust (ADR-0003).

use rust_decimal::Decimal;

use crate::money::{Money, MoneyError, round_half_up};

/// One band of a progressive annual PAYE table. Applies at `rate` to the
/// portion of annual taxable remuneration from `from` up to the next
/// band's `from` (or without upper bound, for the last band).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayeBand {
    from: Money,
    rate: Decimal,
}

impl PayeBand {
    pub fn new(from: Money, rate: Decimal) -> Self {
        PayeBand { from, rate }
    }
}

/// Employee and employer social security contribution rules: a rate each,
/// applied to `BasicPay` clamped between a floor and a ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocialSecurityRules {
    employee_rate: Decimal,
    employer_rate: Decimal,
    floor: Money,
    ceiling: Money,
}

impl SocialSecurityRules {
    pub fn new(
        employee_rate: Decimal,
        employer_rate: Decimal,
        floor: Money,
        ceiling: Money,
    ) -> Self {
        SocialSecurityRules {
            employee_rate,
            employer_rate,
            floor,
            ceiling,
        }
    }

    pub(crate) fn base(self, basic_pay: Money) -> Money {
        basic_pay.clamp(self.floor, self.ceiling)
    }

    pub(crate) fn employee_rate(self) -> Decimal {
        self.employee_rate
    }

    pub(crate) fn employer_rate(self) -> Decimal {
        self.employer_rate
    }

    pub fn floor(self) -> Money {
        self.floor
    }

    pub fn ceiling(self) -> Money {
        self.ceiling
    }
}

/// How an unrounded exact amount becomes a `Money` output line. A field of
/// `PayrollRules` so a change in rounding policy is dated like any other
/// rule; the mechanism itself lives in [`crate::money::round_half_up`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoundingRule {
    HalfUpToCents,
}

impl RoundingRule {
    pub(crate) fn apply(self, amount: Decimal) -> Result<Money, MoneyError> {
        match self {
            RoundingRule::HalfUpToCents => round_half_up(amount),
        }
    }
}

/// The statutory and agreed calculation rules in force for an effective
/// period. Passed beside `PayrollInput`, never inside it, so a test can
/// vary rules against a fixed input (see `calculate`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PayrollRules {
    paye_bands: Vec<PayeBand>,
    social_security: SocialSecurityRules,
    rounding_rule: RoundingRule,
}

impl PayrollRules {
    /// `paye_bands` must be sorted ascending by `from`, with the first
    /// band's `from` at `Money::ZERO`.
    pub fn new(
        paye_bands: Vec<PayeBand>,
        social_security: SocialSecurityRules,
        rounding_rule: RoundingRule,
    ) -> Self {
        PayrollRules {
            paye_bands,
            social_security,
            rounding_rule,
        }
    }

    pub fn social_security(&self) -> SocialSecurityRules {
        self.social_security
    }

    pub fn rounding_rule(&self) -> RoundingRule {
        self.rounding_rule
    }

    /// The exact, unrounded tax owed on `annual_taxable` under the
    /// progressive band table.
    pub(crate) fn tax_owed_on(&self, annual_taxable: Decimal) -> Decimal {
        let mut tax = Decimal::ZERO;
        for (index, band) in self.paye_bands.iter().enumerate() {
            let band_from = band.from.as_decimal();
            if annual_taxable <= band_from {
                break;
            }
            let band_to = self
                .paye_bands
                .get(index + 1)
                .map(|next| next.from.as_decimal());
            let upper = match band_to {
                Some(next_from) => annual_taxable.min(next_from),
                None => annual_taxable,
            };
            tax += (upper - band_from) * band.rate;
        }
        tax
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn money(amount: Decimal) -> Money {
        Money::from_decimal(amount).unwrap()
    }

    // 0-100,000: 0%, 100,000-200,000: 20%, 200,000-400,000: 30%, 400,000+: 40%.
    fn bands() -> Vec<PayeBand> {
        vec![
            PayeBand::new(money(dec!(0)), dec!(0.00)),
            PayeBand::new(money(dec!(100000)), dec!(0.20)),
            PayeBand::new(money(dec!(200000)), dec!(0.30)),
            PayeBand::new(money(dec!(400000)), dec!(0.40)),
        ]
    }

    fn rules() -> PayrollRules {
        PayrollRules::new(
            bands(),
            SocialSecurityRules::new(
                dec!(0.009),
                dec!(0.009),
                money(dec!(500)),
                money(dec!(11000)),
            ),
            RoundingRule::HalfUpToCents,
        )
    }

    #[test]
    fn tax_owed_is_zero_within_the_first_band() {
        assert_eq!(rules().tax_owed_on(dec!(50000)), dec!(0));
    }

    #[test]
    fn tax_owed_is_zero_at_the_top_of_the_first_band() {
        assert_eq!(rules().tax_owed_on(dec!(100000)), dec!(0));
    }

    #[test]
    fn tax_owed_taxes_only_the_portion_within_the_second_band() {
        // 5,000 above the 100,000 threshold at 20%.
        assert_eq!(rules().tax_owed_on(dec!(105000)), dec!(1000.00));
    }

    #[test]
    fn tax_owed_sums_across_every_band_crossed() {
        // 100,000 @ 0% + 100,000 @ 20% + 200,000 @ 30% + 30,000 @ 40%.
        assert_eq!(rules().tax_owed_on(dec!(430000)), dec!(92000.00));
    }

    #[test]
    fn base_clamps_to_the_ceiling() {
        assert_eq!(
            rules().social_security().base(money(dec!(20000))),
            money(dec!(11000))
        );
    }

    #[test]
    fn base_clamps_to_the_floor() {
        assert_eq!(
            rules().social_security().base(money(dec!(300))),
            money(dec!(500))
        );
    }

    #[test]
    fn base_is_unchanged_between_the_floor_and_ceiling() {
        assert_eq!(
            rules().social_security().base(money(dec!(9000))),
            money(dec!(9000))
        );
    }
}
