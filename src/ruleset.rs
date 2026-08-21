//! The statutory `PayrollRules` Salt ships, and `ruleset_for`, the seam
//! that resolves which one is in force for a `PayPeriod` end date
//! (ADR-0005).

use chrono::NaiveDate;
use rust_decimal::Decimal;
use std::sync::LazyLock;

use crate::calculation::PayrollError;
use crate::money::Money;
use crate::rules::{
    EffectivePeriod, PayeBand, PayrollRules, RoundingRule, RulesetId, SocialSecurityRules,
};

fn money(cents: i64) -> Money {
    Money::from_cents(cents).expect("ruleset constants are non-negative literals")
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("ruleset constants are real calendar dates")
}

/// The PAYE band table shared by every ruleset Salt has shipped so far —
/// no ticket has changed it yet, only the SSC ceiling has moved. Kept as
/// its own function so a future ruleset with a different table does not
/// have to duplicate this one.
fn paye_bands() -> Vec<PayeBand> {
    vec![
        PayeBand::new(money(0), Decimal::new(0, 2)).unwrap(),
        PayeBand::new(money(12_000_000), Decimal::new(20, 2)).unwrap(),
        PayeBand::new(money(24_000_000), Decimal::new(30, 2)).unwrap(),
        PayeBand::new(money(48_000_000), Decimal::new(40, 2)).unwrap(),
    ]
}

/// SSC ceiling N$11,000 from 1 March 2025 to 31 August 2026.
fn namibia_2025_march() -> PayrollRules {
    PayrollRules::new(
        RulesetId::new("namibia-2025-03"),
        EffectivePeriod::new(date(2025, 3, 1), Some(date(2026, 8, 31))).unwrap(),
        paye_bands(),
        SocialSecurityRules::new(
            Decimal::new(9, 3),
            Decimal::new(9, 3),
            money(50_000),
            money(1_100_000),
        )
        .unwrap(),
        RoundingRule::HalfUpToCents,
    )
    .expect("namibia-2025-03 is a known-valid statutory constant")
}

/// SSC ceiling N$12,500 from 1 September 2026, open-ended.
fn namibia_2026_september() -> PayrollRules {
    PayrollRules::new(
        RulesetId::new("namibia-2026-09"),
        EffectivePeriod::new(date(2026, 9, 1), None).unwrap(),
        paye_bands(),
        SocialSecurityRules::new(
            Decimal::new(9, 3),
            Decimal::new(9, 3),
            money(50_000),
            money(1_250_000),
        )
        .unwrap(),
        RoundingRule::HalfUpToCents,
    )
    .expect("namibia-2026-09 is a known-valid statutory constant")
}

/// Every `PayrollRules` Salt ships, as typed Rust constants (ADR-0003) —
/// never rows loaded from anywhere. Adding a ruleset means adding an entry
/// here, which is a release.
static KNOWN_RULESETS: LazyLock<[PayrollRules; 2]> =
    LazyLock::new(|| [namibia_2025_march(), namibia_2026_september()]);

/// Resolves the `PayrollRules` in force for a `PayPeriod`'s end date
/// (ADR-0005). A period straddling a change in the rules — e.g. 26 August
/// to 25 September across the 1 September SSC ceiling change — uses
/// whichever ruleset covers its end date, for its entire length: ceilings
/// are monthly amounts, never split pro-rata.
pub fn ruleset_for(period_end: NaiveDate) -> Result<&'static PayrollRules, PayrollError> {
    resolve(&*KNOWN_RULESETS, period_end)
}

/// The selection logic `ruleset_for` runs over the shipped catalogue, over
/// an explicit slice so it can be exercised at the seam with fixtures that
/// deliberately overlap or leave a gap — the shipped catalogue itself
/// never should.
///
/// Every ruleset's `EffectivePeriod` is checked against every other before
/// any match is attempted, so an overlap is detected rather than silently
/// resolved by declaration order or by taking the first match.
fn resolve(
    rulesets: &[PayrollRules],
    period_end: NaiveDate,
) -> Result<&PayrollRules, PayrollError> {
    for (index, earlier) in rulesets.iter().enumerate() {
        for later in &rulesets[index + 1..] {
            if earlier
                .effective_period()
                .overlaps(later.effective_period())
            {
                return Err(PayrollError::OverlappingRulesets {
                    first: earlier.ruleset_id().clone(),
                    second: later.ruleset_id().clone(),
                });
            }
        }
    }
    rulesets
        .iter()
        .find(|rules| rules.effective_period().covers(period_end))
        .ok_or(PayrollError::NoRulesetCoversDate { date: period_end })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_the_ceiling_in_force_before_the_september_change() {
        let rules = ruleset_for(date(2026, 8, 31)).unwrap();
        assert_eq!(rules.ruleset_id().as_str(), "namibia-2025-03");
        assert_eq!(rules.social_security().ceiling(), money(1_100_000));
    }

    #[test]
    fn resolves_the_ceiling_in_force_on_the_september_change() {
        let rules = ruleset_for(date(2026, 9, 1)).unwrap();
        assert_eq!(rules.ruleset_id().as_str(), "namibia-2026-09");
        assert_eq!(rules.social_security().ceiling(), money(1_250_000));
    }

    // A period of 26 August to 25 September 2026 spans the ceiling
    // change, but `ruleset_for` is keyed on the period end alone — there
    // is structurally no way for it to split the period pro-rata.
    #[test]
    fn a_period_straddling_the_change_uses_the_ruleset_of_its_end_date() {
        let period_end = date(2026, 9, 25);
        let rules = ruleset_for(period_end).unwrap();
        assert_eq!(rules.ruleset_id().as_str(), "namibia-2026-09");
        assert_eq!(rules.social_security().ceiling(), money(1_250_000));
    }

    #[test]
    fn refuses_a_date_no_known_ruleset_covers() {
        assert_eq!(
            ruleset_for(date(2020, 1, 1)),
            Err(PayrollError::NoRulesetCoversDate {
                date: date(2020, 1, 1)
            })
        );
    }

    fn fixture(id: &str, from: NaiveDate, until: Option<NaiveDate>) -> PayrollRules {
        PayrollRules::new(
            RulesetId::new(id),
            EffectivePeriod::new(from, until).unwrap(),
            paye_bands(),
            SocialSecurityRules::new(Decimal::new(9, 3), Decimal::new(9, 3), money(0), money(1))
                .unwrap(),
            RoundingRule::HalfUpToCents,
        )
        .unwrap()
    }

    #[test]
    fn refuses_when_two_candidate_rulesets_overlap_rather_than_taking_the_first_match() {
        let earlier = fixture("a", date(2025, 1, 1), Some(date(2025, 12, 31)));
        let later = fixture("b", date(2025, 6, 1), None);

        assert_eq!(
            resolve(&[earlier, later], date(2025, 8, 1)),
            Err(PayrollError::OverlappingRulesets {
                first: RulesetId::new("a"),
                second: RulesetId::new("b"),
            })
        );
    }

    #[test]
    fn refuses_a_date_that_falls_in_a_gap_between_candidate_rulesets() {
        let earlier = fixture("a", date(2025, 1, 1), Some(date(2025, 6, 30)));
        let later = fixture("b", date(2025, 8, 1), None);

        assert_eq!(
            resolve(&[earlier, later], date(2025, 7, 15)),
            Err(PayrollError::NoRulesetCoversDate {
                date: date(2025, 7, 15)
            })
        );
    }

    #[test]
    fn no_two_known_rulesets_overlap() {
        assert!(ruleset_for(date(2025, 3, 1)).is_ok());
        assert!(ruleset_for(date(2026, 9, 1)).is_ok());
    }
}
