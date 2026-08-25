//! The statutory `PayeTable` and `SscRuleset` catalogues Salt ships, and
//! the seams that resolve which one of each is in force for a `PayPeriod`
//! end date: `paye_table_for`, `ssc_rules_for`, and `ruleset_for`, which
//! composes both into one owned `PayrollRules` (ADR-0005, ADR-0007).

use chrono::NaiveDate;
use rust_decimal::Decimal;
use std::sync::LazyLock;

use crate::calculation::PayrollError;
use crate::money::Money;
use crate::rules::{
    EffectivePeriod, PayeBand, PayeTable, PayeTableId, PayrollRules, RoundingRule, SscRulesId,
    SscRuleset,
};

fn money(cents: i64) -> Money {
    Money::from_cents(cents).expect("ruleset constants are non-negative literals")
}

fn date(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).expect("ruleset constants are real calendar dates")
}

/// The PAYE band table shared by every `PayeTable` Salt has shipped so
/// far — no ticket has changed it yet. Kept as its own function so a
/// future table does not have to duplicate this one.
fn paye_bands() -> Vec<PayeBand> {
    vec![
        PayeBand::new(money(0), Decimal::new(0, 2)).unwrap(),
        PayeBand::new(money(12_000_000), Decimal::new(20, 2)).unwrap(),
        PayeBand::new(money(24_000_000), Decimal::new(30, 2)).unwrap(),
        PayeBand::new(money(48_000_000), Decimal::new(40, 2)).unwrap(),
    ]
}

/// The one `PayeTable` Salt has shipped so far — synthetic figures, not
/// yet the real NamRA table (that is the next ticket on GitHub issue #7).
/// Structural only: it resolves on its own axis via `paye_table_for`,
/// entirely independent of `ssc_rules_for` (ADR-0007).
fn namibia_synthetic() -> PayeTable {
    PayeTable::new(
        PayeTableId::new("namibia-synthetic"),
        paye_bands(),
        date(2025, 3, 1),
        EffectivePeriod::new(date(2025, 3, 1), None).unwrap(),
    )
    .expect("namibia-synthetic is a known-valid statutory constant")
}

/// Every `PayeTable` Salt ships, as typed Rust constants (ADR-0003) —
/// never rows loaded from anywhere. Adding a table means adding an entry
/// here, which is a release.
static KNOWN_PAYE_TABLES: LazyLock<[PayeTable; 1]> = LazyLock::new(|| [namibia_synthetic()]);

/// SSC ceiling N$11,000/month from 1 March 2025 to 31 August 2026
/// (`docs/conformance/ssc-2025-03.md`).
fn ssc_2025_march() -> SscRuleset {
    SscRuleset::new(
        SscRulesId::new("ssc-2025-03"),
        Decimal::new(9, 3),
        Decimal::new(9, 3),
        money(50_000),
        money(1_100_000),
        date(2025, 3, 1),
        EffectivePeriod::new(date(2025, 3, 1), Some(date(2026, 8, 31))).unwrap(),
    )
    .expect("ssc-2025-03 is a known-valid statutory constant")
}

/// SSC ceiling N$12,500/month from the September 2026 payroll, open-ended
/// (`docs/conformance/ssc-2026-09.md`). Government Notice 236 states a
/// legal effective date of 1 March 2026, but was gazetted 15 July 2026 —
/// after that date — and the Social Security Commission confirmed
/// implementation from the September 2026 payroll with no retrospective
/// adjustment. `legal_effective_from` records what the instrument says;
/// `payroll_effective_from` (the start of `payroll_applicability`) is what
/// `ssc_rules_for` actually resolves on.
fn ssc_2026_september() -> SscRuleset {
    SscRuleset::new(
        SscRulesId::new("ssc-2026-09"),
        Decimal::new(9, 3),
        Decimal::new(9, 3),
        money(50_000),
        money(1_250_000),
        date(2026, 3, 1),
        EffectivePeriod::new(date(2026, 9, 1), None).unwrap(),
    )
    .expect("ssc-2026-09 is a known-valid statutory constant")
}

/// Every `SscRuleset` Salt ships, as typed Rust constants (ADR-0003) —
/// never rows loaded from anywhere. Adding a ruleset means adding an entry
/// here, which is a release.
static KNOWN_SSC_RULESETS: LazyLock<[SscRuleset; 2]> =
    LazyLock::new(|| [ssc_2025_march(), ssc_2026_september()]);

/// A catalogue entry that resolves on its own `payroll_applicability`
/// interval. Implemented once for `PayeTable` and once for `SscRuleset` so
/// `resolve` runs identically over either catalogue, rather than each axis
/// inventing its own overlap/gap logic (ADR-0007).
trait Resolvable {
    type Id: Clone;

    fn resolved_id(&self) -> Self::Id;
    fn resolved_applicability(&self) -> EffectivePeriod;
}

impl Resolvable for PayeTable {
    type Id = PayeTableId;

    fn resolved_id(&self) -> PayeTableId {
        self.id().clone()
    }

    fn resolved_applicability(&self) -> EffectivePeriod {
        self.payroll_applicability()
    }
}

impl Resolvable for SscRuleset {
    type Id = SscRulesId;

    fn resolved_id(&self) -> SscRulesId {
        self.id().clone()
    }

    fn resolved_applicability(&self) -> EffectivePeriod {
        self.payroll_applicability()
    }
}

/// Why `resolve` could not select a single catalogue entry, generic over
/// the catalogue's own id type so one function serves both axes; each
/// caller maps this to the `PayrollError` variant naming its own
/// catalogue (ADR-0007's "a catalogue mistake now says which catalogue is
/// wrong").
#[derive(Debug, Clone, PartialEq, Eq)]
enum ResolveError<Id> {
    Overlap(Id, Id),
    NoCoverage(NaiveDate),
}

/// The selection logic `paye_table_for` and `ssc_rules_for` each run over
/// their own shipped catalogue, over an explicit slice so it can be
/// exercised at the seam with fixtures that deliberately overlap or leave
/// a gap — the shipped catalogues themselves never should.
///
/// Every entry's `payroll_applicability` is checked against every other's
/// before any match is attempted, so an overlap is detected rather than
/// silently resolved by declaration order or by taking the first match.
fn resolve<T: Resolvable>(entries: &[T], period_end: NaiveDate) -> Result<&T, ResolveError<T::Id>> {
    for (index, earlier) in entries.iter().enumerate() {
        for later in &entries[index + 1..] {
            if earlier
                .resolved_applicability()
                .overlaps(later.resolved_applicability())
            {
                return Err(ResolveError::Overlap(
                    earlier.resolved_id(),
                    later.resolved_id(),
                ));
            }
        }
    }
    entries
        .iter()
        .find(|entry| entry.resolved_applicability().covers(period_end))
        .ok_or(ResolveError::NoCoverage(period_end))
}

/// Resolves the `PayeTable` in force for a `PayPeriod`'s end date
/// (ADR-0005, ADR-0007), independent of `ssc_rules_for`.
pub fn paye_table_for(period_end: NaiveDate) -> Result<&'static PayeTable, PayrollError> {
    resolve(&*KNOWN_PAYE_TABLES, period_end).map_err(|err| match err {
        ResolveError::Overlap(first, second) => {
            PayrollError::OverlappingPayeTables { first, second }
        }
        ResolveError::NoCoverage(date) => PayrollError::NoPayeTableCoversDate { date },
    })
}

/// Resolves the `SscRuleset` in force for a `PayPeriod`'s end date
/// (ADR-0005, ADR-0007), independent of `paye_table_for`.
pub fn ssc_rules_for(period_end: NaiveDate) -> Result<&'static SscRuleset, PayrollError> {
    resolve(&*KNOWN_SSC_RULESETS, period_end).map_err(|err| match err {
        ResolveError::Overlap(first, second) => {
            PayrollError::OverlappingSscRulesets { first, second }
        }
        ResolveError::NoCoverage(date) => PayrollError::NoSscRulesetCoversDate { date },
    })
}

/// Resolves the `PayeTable` and `SscRuleset` in force for a `PayPeriod`'s
/// end date, each on its own axis, and freezes both into one owned
/// `PayrollRules` (ADR-0007) — the one seam whose return type intentionally
/// moved from `&'static PayrollRules` to an owned value, since once the two
/// halves resolve independently there is no single static value left to
/// borrow. A period straddling a change in either axis — e.g. 26 August to
/// 25 September across the 1 September SSC ceiling change — uses whichever
/// entry covers its end date, for its entire length: ceilings are monthly
/// amounts, never split pro-rata.
pub fn ruleset_for(period_end: NaiveDate) -> Result<PayrollRules, PayrollError> {
    let paye_table = paye_table_for(period_end)?.clone();
    let ssc_ruleset = ssc_rules_for(period_end)?.clone();
    Ok(
        PayrollRules::new(paye_table, ssc_ruleset, RoundingRule::HalfUpToCents)
            .expect("composing already-validated catalogue entries cannot fail"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_the_ceiling_in_force_before_the_september_change() {
        let rules = ruleset_for(date(2026, 8, 31)).unwrap();
        assert_eq!(rules.ssc_ruleset().id().as_str(), "ssc-2025-03");
        assert_eq!(rules.social_security().ceiling(), money(1_100_000));
    }

    #[test]
    fn resolves_the_ceiling_in_force_on_the_september_change() {
        let rules = ruleset_for(date(2026, 9, 1)).unwrap();
        assert_eq!(rules.ssc_ruleset().id().as_str(), "ssc-2026-09");
        assert_eq!(rules.social_security().ceiling(), money(1_250_000));
    }

    // A period of 26 August to 25 September 2026 spans the ceiling
    // change, but `ssc_rules_for` is keyed on the period end alone — there
    // is structurally no way for it to split the period pro-rata.
    #[test]
    fn a_period_straddling_the_change_uses_the_ruleset_of_its_end_date() {
        let period_end = date(2026, 9, 25);
        let rules = ruleset_for(period_end).unwrap();
        assert_eq!(rules.ssc_ruleset().id().as_str(), "ssc-2026-09");
        assert_eq!(rules.social_security().ceiling(), money(1_250_000));
    }

    // ADR-0007: `legal_effective_from` is carried, stored and displayed
    // but never used for selection. `ssc-2026-09` is the one shipped
    // ruleset whose two dates differ — its legal date is 1 March 2026, but
    // a period ending in March 2026 still resolves the *previous*
    // ruleset, because payroll applicability (not the legal date) is what
    // `ssc_rules_for` selects on.
    #[test]
    fn legal_effective_from_is_never_used_for_selection() {
        let rules = ruleset_for(date(2026, 3, 1)).unwrap();
        assert_eq!(rules.ssc_ruleset().id().as_str(), "ssc-2025-03");
        assert_eq!(
            ssc_rules_for(date(2026, 3, 1))
                .unwrap()
                .legal_effective_from(),
            date(2025, 3, 1),
            "the resolved ruleset is ssc-2025-03, whose own legal date is unrelated to ssc-2026-09's"
        );

        let superseding = ssc_rules_for(date(2026, 9, 1)).unwrap();
        assert_eq!(superseding.legal_effective_from(), date(2026, 3, 1));
        assert_eq!(superseding.payroll_effective_from(), date(2026, 9, 1));
    }

    #[test]
    fn refuses_a_date_no_known_paye_table_or_ssc_ruleset_covers() {
        assert_eq!(
            ruleset_for(date(2020, 1, 1)),
            Err(PayrollError::NoPayeTableCoversDate {
                date: date(2020, 1, 1)
            })
        );
    }

    fn paye_table_fixture(id: &str, applicability: EffectivePeriod) -> PayeTable {
        PayeTable::new(
            PayeTableId::new(id),
            paye_bands(),
            applicability.from(),
            applicability,
        )
        .unwrap()
    }

    fn ssc_ruleset_fixture(id: &str, applicability: EffectivePeriod) -> SscRuleset {
        SscRuleset::new(
            SscRulesId::new(id),
            Decimal::new(9, 3),
            Decimal::new(9, 3),
            money(0),
            money(1),
            applicability.from(),
            applicability,
        )
        .unwrap()
    }

    #[test]
    fn refuses_when_two_candidate_paye_tables_overlap_rather_than_taking_the_first_match() {
        let earlier = paye_table_fixture(
            "a",
            EffectivePeriod::new(date(2025, 1, 1), Some(date(2025, 12, 31))).unwrap(),
        );
        let later = paye_table_fixture("b", EffectivePeriod::new(date(2025, 6, 1), None).unwrap());

        assert_eq!(
            resolve(&[earlier, later], date(2025, 8, 1)),
            Err(ResolveError::Overlap(
                PayeTableId::new("a"),
                PayeTableId::new("b"),
            ))
        );
    }

    #[test]
    fn refuses_when_two_candidate_ssc_rulesets_overlap_rather_than_taking_the_first_match() {
        let earlier = ssc_ruleset_fixture(
            "a",
            EffectivePeriod::new(date(2025, 1, 1), Some(date(2025, 12, 31))).unwrap(),
        );
        let later = ssc_ruleset_fixture("b", EffectivePeriod::new(date(2025, 6, 1), None).unwrap());

        assert_eq!(
            resolve(&[earlier, later], date(2025, 8, 1)),
            Err(ResolveError::Overlap(
                SscRulesId::new("a"),
                SscRulesId::new("b"),
            ))
        );
    }

    #[test]
    fn refuses_a_date_that_falls_in_a_gap_between_candidate_entries() {
        let earlier = ssc_ruleset_fixture(
            "a",
            EffectivePeriod::new(date(2025, 1, 1), Some(date(2025, 6, 30))).unwrap(),
        );
        let later = ssc_ruleset_fixture("b", EffectivePeriod::new(date(2025, 8, 1), None).unwrap());

        assert_eq!(
            resolve(&[earlier, later], date(2025, 7, 15)),
            Err(ResolveError::NoCoverage(date(2025, 7, 15)))
        );
    }

    #[test]
    fn refuses_the_day_before_the_earliest_known_entry_starts() {
        assert_eq!(
            ruleset_for(date(2025, 2, 28)),
            Err(PayrollError::NoPayeTableCoversDate {
                date: date(2025, 2, 28)
            })
        );
    }

    // `resolve` compares every pair, not only neighbours, so an overlap
    // between entries that are not adjacent in declaration order is still
    // detected rather than hidden by a nearer non-overlapping pair.
    #[test]
    fn refuses_an_overlap_between_entries_that_are_not_adjacent() {
        let first = ssc_ruleset_fixture(
            "a",
            EffectivePeriod::new(date(2025, 1, 1), Some(date(2027, 12, 31))).unwrap(),
        );
        let second = ssc_ruleset_fixture(
            "b",
            EffectivePeriod::new(date(2020, 1, 1), Some(date(2020, 12, 31))).unwrap(),
        );
        let third = ssc_ruleset_fixture(
            "c",
            EffectivePeriod::new(date(2026, 1, 1), Some(date(2026, 12, 31))).unwrap(),
        );

        assert_eq!(
            resolve(&[first, second, third], date(2026, 6, 1)),
            Err(ResolveError::Overlap(
                SscRulesId::new("a"),
                SscRulesId::new("c"),
            ))
        );
    }

    // The shipped PAYE catalogue is the one set `paye_table_for` can never
    // be given a fixture for, so its coherence is asserted directly here.
    #[test]
    fn the_shipped_paye_catalogue_is_ordered_contiguous_and_free_of_overlaps() {
        assert_shipped_catalogue_coherent(&*KNOWN_PAYE_TABLES, |t| t.id().to_string());
    }

    // Same coherence property, asserted for the independent SSC axis.
    #[test]
    fn the_shipped_ssc_catalogue_is_ordered_contiguous_and_free_of_overlaps() {
        assert_shipped_catalogue_coherent(&*KNOWN_SSC_RULESETS, |r| r.id().to_string());
    }

    /// Without this, adding a `PayeTable` or `SscRuleset` that overlaps or
    /// leaves a gap would compile and ship, and fail for the first
    /// employer to run a payslip rather than for the release that
    /// introduced it. Generic so both catalogues run the identical check.
    fn assert_shipped_catalogue_coherent<T: Resolvable>(
        catalogue: &[T],
        label: impl Fn(&T) -> String,
    ) {
        for (index, earlier) in catalogue.iter().enumerate() {
            for later in &catalogue[index + 1..] {
                assert!(
                    !earlier
                        .resolved_applicability()
                        .overlaps(later.resolved_applicability()),
                    "{} overlaps {}",
                    label(earlier),
                    label(later)
                );
            }
        }

        for pair in catalogue.windows(2) {
            let (earlier, later) = (
                pair[0].resolved_applicability(),
                pair[1].resolved_applicability(),
            );
            let ends = earlier
                .until()
                .expect("only the newest shipped entry may be open-ended");
            assert!(
                ends < later.from(),
                "shipped entries are declared oldest first"
            );
            assert_eq!(
                ends.succ_opt(),
                Some(later.from()),
                "a gap between shipped entries would leave a date uncovered"
            );
        }

        let newest = catalogue
            .last()
            .expect("the shipped catalogue is never empty");
        assert_eq!(
            newest.resolved_applicability().until(),
            None,
            "the newest shipped entry stays in force until a release replaces it"
        );
    }

    // Only the SSC ceiling has moved between the shipped rulesets. The
    // floor and both rates are asserted on every one of them so a future
    // ruleset cannot change them by accident.
    #[test]
    fn every_shipped_ssc_ruleset_carries_the_statutory_floor_and_rates() {
        for ruleset in &*KNOWN_SSC_RULESETS {
            let ssc = ruleset.social_security();
            assert_eq!(ssc.floor(), money(50_000), "{}", ruleset.id());
            assert_eq!(ssc.employee_rate(), Decimal::new(9, 3), "{}", ruleset.id());
            assert_eq!(ssc.employer_rate(), Decimal::new(9, 3), "{}", ruleset.id());
        }
    }

    #[test]
    fn ruleset_for_records_both_ids_it_resolved() {
        let rules = ruleset_for(date(2026, 9, 1)).unwrap();
        assert_eq!(rules.paye_table().id().as_str(), "namibia-synthetic");
        assert_eq!(rules.ssc_ruleset().id().as_str(), "ssc-2026-09");
    }

    #[test]
    fn a_shipped_ruleset_round_trips_through_serde() {
        let rules = ruleset_for(date(2026, 9, 1)).unwrap();
        let json = serde_json::to_string(&rules).unwrap();
        assert_eq!(serde_json::from_str::<PayrollRules>(&json).unwrap(), rules);
    }
}
