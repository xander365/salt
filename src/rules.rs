//! `PayrollRules`: the statutory and agreed calculation rules in force for
//! an effective period, as typed Rust (ADR-0003).

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::money::{Money, MoneyError};

/// Why a `PayrollRules` component could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayrollRulesError {
    /// A PAYE band or social security rate was negative.
    NegativeRate,
    /// The PAYE band table had no bands at all.
    EmptyBandTable,
    /// The first PAYE band did not start at `Money::ZERO`.
    FirstBandNotZero,
    /// The PAYE band table was not strictly ascending by `from`.
    BandsNotAscending,
    /// The social security floor was above its ceiling.
    FloorAboveCeiling,
}

impl std::fmt::Display for PayrollRulesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PayrollRulesError::NegativeRate => write!(f, "a rate was negative"),
            PayrollRulesError::EmptyBandTable => write!(f, "the PAYE band table was empty"),
            PayrollRulesError::FirstBandNotZero => {
                write!(f, "the first PAYE band did not start at zero")
            }
            PayrollRulesError::BandsNotAscending => {
                write!(f, "the PAYE band table was not strictly ascending")
            }
            PayrollRulesError::FloorAboveCeiling => {
                write!(f, "the social security floor was above its ceiling")
            }
        }
    }
}

impl std::error::Error for PayrollRulesError {}

/// One band of a progressive annual PAYE table. Applies at `rate` to the
/// portion of annual taxable remuneration from `from` up to the next
/// band's `from` (or without upper bound, for the last band).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawPayeBand", into = "RawPayeBand")]
pub struct PayeBand {
    from: Money,
    rate: Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawPayeBand {
    pub from: Money,
    pub rate: Decimal,
}

impl PayeBand {
    pub fn new(from: Money, rate: Decimal) -> Result<Self, PayrollRulesError> {
        if rate.is_sign_negative() {
            return Err(PayrollRulesError::NegativeRate);
        }
        Ok(PayeBand { from, rate })
    }
}

impl TryFrom<RawPayeBand> for PayeBand {
    type Error = PayrollRulesError;

    fn try_from(raw: RawPayeBand) -> Result<Self, PayrollRulesError> {
        PayeBand::new(raw.from, raw.rate)
    }
}

impl From<PayeBand> for RawPayeBand {
    fn from(band: PayeBand) -> RawPayeBand {
        RawPayeBand {
            from: band.from,
            rate: band.rate,
        }
    }
}

/// Whether a social security base was clamped to its floor or ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SscClamp {
    None,
    Floor,
    Ceiling,
}

/// Employee and employer social security contribution rules: a rate each,
/// applied to `BasicPay` clamped between a floor and a ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawSocialSecurityRules", into = "RawSocialSecurityRules")]
pub struct SocialSecurityRules {
    employee_rate: Decimal,
    employer_rate: Decimal,
    floor: Money,
    ceiling: Money,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawSocialSecurityRules {
    pub employee_rate: Decimal,
    pub employer_rate: Decimal,
    pub floor: Money,
    pub ceiling: Money,
}

impl SocialSecurityRules {
    pub fn new(
        employee_rate: Decimal,
        employer_rate: Decimal,
        floor: Money,
        ceiling: Money,
    ) -> Result<Self, PayrollRulesError> {
        if employee_rate.is_sign_negative() || employer_rate.is_sign_negative() {
            return Err(PayrollRulesError::NegativeRate);
        }
        if floor > ceiling {
            return Err(PayrollRulesError::FloorAboveCeiling);
        }
        Ok(SocialSecurityRules {
            employee_rate,
            employer_rate,
            floor,
            ceiling,
        })
    }

    pub(crate) fn base(self, basic_pay: Money) -> (Money, SscClamp) {
        if basic_pay < self.floor {
            (self.floor, SscClamp::Floor)
        } else if basic_pay > self.ceiling {
            (self.ceiling, SscClamp::Ceiling)
        } else {
            (basic_pay, SscClamp::None)
        }
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

impl TryFrom<RawSocialSecurityRules> for SocialSecurityRules {
    type Error = PayrollRulesError;

    fn try_from(raw: RawSocialSecurityRules) -> Result<Self, PayrollRulesError> {
        SocialSecurityRules::new(raw.employee_rate, raw.employer_rate, raw.floor, raw.ceiling)
    }
}

impl From<SocialSecurityRules> for RawSocialSecurityRules {
    fn from(rules: SocialSecurityRules) -> RawSocialSecurityRules {
        RawSocialSecurityRules {
            employee_rate: rules.employee_rate,
            employer_rate: rules.employer_rate,
            floor: rules.floor,
            ceiling: rules.ceiling,
        }
    }
}

/// How an unrounded exact amount becomes a `Money` output line. A field of
/// `PayrollRules` so a change in rounding policy is dated like any other
/// rule; the mechanism itself lives in [`crate::money::round_half_up`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoundingRule {
    HalfUpToCents,
}

impl RoundingRule {
    pub(crate) fn apply(self, amount: Decimal) -> Result<Money, crate::money::MoneyError> {
        match self {
            RoundingRule::HalfUpToCents => crate::money::round_half_up(amount),
        }
    }
}

/// One PAYE band's contribution to the tax owed on a year-to-date taxable
/// amount, for explainability. `threshold` is the band's annual `from`
/// scaled to the period number within the TaxYear (see
/// [`PayrollRules::tax_owed_on`]);
/// intermediate arithmetic like this is never rounded to cents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BandContribution {
    pub threshold: Decimal,
    pub rate: Decimal,
    pub tax: Decimal,
}

/// The statutory and agreed calculation rules in force for an effective
/// period. Passed beside `PayrollInput`, never inside it, so a test can
/// vary rules against a fixed input (see `calculate`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawPayrollRules", into = "RawPayrollRules")]
pub struct PayrollRules {
    paye_bands: Vec<PayeBand>,
    social_security: SocialSecurityRules,
    rounding_rule: RoundingRule,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawPayrollRules {
    pub paye_bands: Vec<PayeBand>,
    pub social_security: SocialSecurityRules,
    pub rounding_rule: RoundingRule,
}

impl PayrollRules {
    /// `paye_bands` must be non-empty, strictly ascending by `from`, with
    /// the first band's `from` at `Money::ZERO`.
    pub fn new(
        paye_bands: Vec<PayeBand>,
        social_security: SocialSecurityRules,
        rounding_rule: RoundingRule,
    ) -> Result<Self, PayrollRulesError> {
        let Some(first) = paye_bands.first() else {
            return Err(PayrollRulesError::EmptyBandTable);
        };
        if first.from != Money::ZERO {
            return Err(PayrollRulesError::FirstBandNotZero);
        }
        if paye_bands
            .windows(2)
            .any(|pair| pair[1].from <= pair[0].from)
        {
            return Err(PayrollRulesError::BandsNotAscending);
        }
        Ok(PayrollRules {
            paye_bands,
            social_security,
            rounding_rule,
        })
    }

    pub fn social_security(&self) -> SocialSecurityRules {
        self.social_security
    }

    pub fn rounding_rule(&self) -> RoundingRule {
        self.rounding_rule
    }

    /// The exact, unrounded tax owed on `annual_taxable`, and the band-by-
    /// band breakdown that produced it.
    ///
    /// Cumulative PAYE is never annualised (ADR-0001): rather than scale
    /// `annual_taxable` up to a full-year estimate, this scales the annual
    /// band *thresholds* down to `period_number`/12 of their
    /// full value before taxing the actual year-to-date amount against
    /// them. At period 12 the thresholds equal the full annual table,
    /// giving every employer exactly 12 periods that reconcile to the
    /// standard annual calculation; earlier in the year the same
    /// cumulative income can therefore land in a different band than it
    /// would later on.
    pub(crate) fn tax_owed_on(
        &self,
        annual_taxable: Decimal,
        period_number: u32,
    ) -> Result<(Decimal, Vec<BandContribution>), MoneyError> {
        let elapsed = Decimal::from(period_number);
        let twelve = Decimal::from(12u32);
        // Every step is checked: `Decimal`'s operators panic on overflow,
        // and a rules table is data a caller supplies.
        let scaled_threshold = |from: Money| -> Result<Decimal, MoneyError> {
            from.as_decimal()
                .checked_mul(elapsed)
                .and_then(|scaled| scaled.checked_div(twelve))
                .ok_or(MoneyError::Overflow)
        };

        let mut total = Decimal::ZERO;
        let mut contributions = Vec::new();
        for (index, band) in self.paye_bands.iter().enumerate() {
            let threshold = scaled_threshold(band.from)?;
            if annual_taxable <= threshold {
                break;
            }
            let upper = match self.paye_bands.get(index + 1) {
                Some(next) => annual_taxable.min(scaled_threshold(next.from)?),
                None => annual_taxable,
            };
            let tax = upper
                .checked_sub(threshold)
                .and_then(|span| span.checked_mul(band.rate))
                .ok_or(MoneyError::Overflow)?;
            total = total.checked_add(tax).ok_or(MoneyError::Overflow)?;
            contributions.push(BandContribution {
                threshold,
                rate: band.rate,
                tax,
            });
        }
        Ok((total, contributions))
    }
}

impl TryFrom<RawPayrollRules> for PayrollRules {
    type Error = PayrollRulesError;

    fn try_from(raw: RawPayrollRules) -> Result<Self, PayrollRulesError> {
        PayrollRules::new(raw.paye_bands, raw.social_security, raw.rounding_rule)
    }
}

impl From<PayrollRules> for RawPayrollRules {
    fn from(rules: PayrollRules) -> RawPayrollRules {
        RawPayrollRules {
            paye_bands: rules.paye_bands,
            social_security: rules.social_security,
            rounding_rule: rules.rounding_rule,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn money(amount: Decimal) -> Money {
        Money::from_decimal(amount).unwrap()
    }

    // Thresholds are multiples of 12 so that scaling by
    // periods_elapsed_inclusive/12 stays exact decimal arithmetic at every
    // period, matching how Money itself is exact: 0-120,000: 0%,
    // 120,000-240,000: 20%, 240,000-480,000: 30%, 480,000+: 40%.
    fn bands() -> Vec<PayeBand> {
        vec![
            PayeBand::new(money(dec!(0)), dec!(0.00)).unwrap(),
            PayeBand::new(money(dec!(120000)), dec!(0.20)).unwrap(),
            PayeBand::new(money(dec!(240000)), dec!(0.30)).unwrap(),
            PayeBand::new(money(dec!(480000)), dec!(0.40)).unwrap(),
        ]
    }

    fn social_security() -> SocialSecurityRules {
        SocialSecurityRules::new(
            dec!(0.009),
            dec!(0.009),
            money(dec!(500)),
            money(dec!(11000)),
        )
        .unwrap()
    }

    fn rules() -> PayrollRules {
        PayrollRules::new(bands(), social_security(), RoundingRule::HalfUpToCents).unwrap()
    }

    #[test]
    fn tax_owed_is_zero_within_the_first_band() {
        assert_eq!(rules().tax_owed_on(dec!(50000), 12).unwrap().0, dec!(0));
    }

    #[test]
    fn tax_owed_is_zero_at_the_top_of_the_first_band() {
        assert_eq!(rules().tax_owed_on(dec!(120000), 12).unwrap().0, dec!(0));
    }

    #[test]
    fn tax_owed_taxes_only_the_portion_within_the_second_band() {
        // At period 12 (full annual thresholds): 5,000 above the 120,000
        // threshold at 20%.
        assert_eq!(
            rules().tax_owed_on(dec!(125000), 12).unwrap().0,
            dec!(1000.00)
        );
    }

    #[test]
    fn tax_owed_sums_across_every_band_crossed() {
        // 120,000 @ 0% + 120,000 @ 20% + 240,000 @ 30% + 30,000 @ 40%.
        assert_eq!(
            rules().tax_owed_on(dec!(510000), 12).unwrap().0,
            dec!(108000.00)
        );
    }

    #[test]
    fn same_ytd_taxable_owes_more_later_in_the_tax_year() {
        // At period 3, the first band's threshold is scaled to 120,000 *
        // 3/12 = 30,000, so 50,000 already reaches the second band.
        let (early, _) = rules().tax_owed_on(dec!(50000), 3).unwrap();
        assert_eq!(early, dec!(4000.00));

        // At period 6, the threshold is scaled to 120,000 * 6/12 = 60,000,
        // so the same 50,000 has not reached it yet.
        let (later, _) = rules().tax_owed_on(dec!(50000), 6).unwrap();
        assert_eq!(later, dec!(0));

        assert_ne!(early, later);
    }

    #[test]
    fn band_contributions_report_the_scaled_threshold_and_rate() {
        // 50,000 at period 3 reaches into the second band, so both the
        // (zero-tax) first band and the second band contribute an entry.
        let (_, contributions) = rules().tax_owed_on(dec!(50000), 3).unwrap();
        assert_eq!(contributions.len(), 2);
        assert_eq!(contributions[0].threshold, dec!(0));
        assert_eq!(contributions[0].rate, dec!(0.00));
        assert_eq!(contributions[0].tax, dec!(0));
        assert_eq!(contributions[1].threshold, dec!(30000));
        assert_eq!(contributions[1].rate, dec!(0.20));
        assert_eq!(contributions[1].tax, dec!(4000.00));
    }

    #[test]
    fn base_clamps_to_the_ceiling() {
        assert_eq!(
            social_security().base(money(dec!(20000))),
            (money(dec!(11000)), SscClamp::Ceiling)
        );
    }

    #[test]
    fn base_clamps_to_the_floor() {
        assert_eq!(
            social_security().base(money(dec!(300))),
            (money(dec!(500)), SscClamp::Floor)
        );
    }

    #[test]
    fn base_is_unchanged_between_the_floor_and_ceiling() {
        assert_eq!(
            social_security().base(money(dec!(9000))),
            (money(dec!(9000)), SscClamp::None)
        );
    }

    #[test]
    fn rejects_a_negative_paye_band_rate() {
        assert_eq!(
            PayeBand::new(money(dec!(0)), dec!(-0.01)),
            Err(PayrollRulesError::NegativeRate)
        );
    }

    #[test]
    fn rejects_a_negative_social_security_rate() {
        assert_eq!(
            SocialSecurityRules::new(
                dec!(-0.01),
                dec!(0.009),
                money(dec!(500)),
                money(dec!(11000))
            ),
            Err(PayrollRulesError::NegativeRate)
        );
    }

    #[test]
    fn rejects_a_floor_above_its_ceiling() {
        assert_eq!(
            SocialSecurityRules::new(
                dec!(0.009),
                dec!(0.009),
                money(dec!(11000)),
                money(dec!(500))
            ),
            Err(PayrollRulesError::FloorAboveCeiling)
        );
    }

    #[test]
    fn rejects_an_empty_band_table() {
        assert_eq!(
            PayrollRules::new(Vec::new(), social_security(), RoundingRule::HalfUpToCents),
            Err(PayrollRulesError::EmptyBandTable)
        );
    }

    #[test]
    fn rejects_a_first_band_that_does_not_start_at_zero() {
        let bands = vec![PayeBand::new(money(dec!(100)), dec!(0.20)).unwrap()];
        assert_eq!(
            PayrollRules::new(bands, social_security(), RoundingRule::HalfUpToCents),
            Err(PayrollRulesError::FirstBandNotZero)
        );
    }

    #[test]
    fn rejects_bands_that_are_not_strictly_ascending() {
        let bands = vec![
            PayeBand::new(money(dec!(0)), dec!(0.00)).unwrap(),
            PayeBand::new(money(dec!(100)), dec!(0.20)).unwrap(),
            PayeBand::new(money(dec!(100)), dec!(0.30)).unwrap(),
        ];
        assert_eq!(
            PayrollRules::new(bands, social_security(), RoundingRule::HalfUpToCents),
            Err(PayrollRulesError::BandsNotAscending)
        );
    }

    #[test]
    fn deserialize_round_trips() {
        let rules = rules();
        let json = serde_json::to_string(&rules).unwrap();
        assert_eq!(serde_json::from_str::<PayrollRules>(&json).unwrap(), rules);
    }

    #[test]
    fn deserialize_rejects_an_unsorted_band_table() {
        let json = serde_json::to_string(&RawPayrollRules {
            paye_bands: vec![
                PayeBand::new(money(dec!(0)), dec!(0.0)).unwrap(),
                PayeBand::new(money(dec!(200)), dec!(0.1)).unwrap(),
                PayeBand::new(money(dec!(100)), dec!(0.2)).unwrap(),
            ],
            social_security: social_security(),
            rounding_rule: RoundingRule::HalfUpToCents,
        })
        .unwrap();
        assert!(serde_json::from_str::<PayrollRules>(&json).is_err());
    }
}
