//! `PayrollRules`: the statutory and agreed calculation rules Salt ships,
//! composed from an independently-resolved `PayeTable` and `SscRuleset`
//! (ADR-0007), as typed Rust (ADR-0003).

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::calculation::PayrollError;
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
    /// An `EffectivePeriod`'s `until` date was before its `from` date.
    EffectivePeriodEndsBeforeItStarts,
    /// A `PayeTable` or `SscRuleset` claimed to apply to payroll before
    /// the date its own instrument says it takes legal effect. The two
    /// dates exist to record a *deferral* — payroll applying a rule later
    /// than the law does, as `ssc-2026-09` does (ADR-0007). The reverse
    /// has no justification: it would withhold under a rule that did not
    /// yet exist.
    PayrollAppliesBeforeLegalEffectiveDate,
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
            PayrollRulesError::EffectivePeriodEndsBeforeItStarts => {
                write!(
                    f,
                    "the effective period's until date is before its from date"
                )
            }
            PayrollRulesError::PayrollAppliesBeforeLegalEffectiveDate => {
                write!(
                    f,
                    "payroll applicability starts before the legal effective date"
                )
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

/// Identifies a `PayeTable`, independent of any `SscRulesId`: PAYE and
/// social security move on separate effective-date axes (ADR-0007).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PayeTableId(String);

impl PayeTableId {
    pub fn new(id: impl Into<String>) -> Self {
        PayeTableId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PayeTableId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A progressive annual PAYE band table, with its own identity and its own
/// pair of effective dates (ADR-0007): `legal_effective_from` is what the
/// instrument says, `payroll_effective_from` — the start of
/// `payroll_applicability` — is what payroll actually applies.
/// `paye_table_for` resolves a `PayeTable` on this axis alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawPayeTable", into = "RawPayeTable")]
pub struct PayeTable {
    id: PayeTableId,
    bands: Vec<PayeBand>,
    legal_effective_from: NaiveDate,
    payroll_applicability: EffectivePeriod,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawPayeTable {
    pub id: PayeTableId,
    pub bands: Vec<PayeBand>,
    pub legal_effective_from: NaiveDate,
    pub payroll_applicability: EffectivePeriod,
}

impl PayeTable {
    /// `bands` must be non-empty, strictly ascending by `from`, with the
    /// first band's `from` at `Money::ZERO`, and `payroll_applicability`
    /// must not start before `legal_effective_from` (ADR-0007: the two
    /// dates record a deferral, never a retro-active application).
    pub fn new(
        id: PayeTableId,
        bands: Vec<PayeBand>,
        legal_effective_from: NaiveDate,
        payroll_applicability: EffectivePeriod,
    ) -> Result<Self, PayrollRulesError> {
        let Some(first) = bands.first() else {
            return Err(PayrollRulesError::EmptyBandTable);
        };
        if first.from != Money::ZERO {
            return Err(PayrollRulesError::FirstBandNotZero);
        }
        if bands.windows(2).any(|pair| pair[1].from <= pair[0].from) {
            return Err(PayrollRulesError::BandsNotAscending);
        }
        if payroll_applicability.from() < legal_effective_from {
            return Err(PayrollRulesError::PayrollAppliesBeforeLegalEffectiveDate);
        }
        Ok(PayeTable {
            id,
            bands,
            legal_effective_from,
            payroll_applicability,
        })
    }

    pub fn id(&self) -> &PayeTableId {
        &self.id
    }

    pub fn bands(&self) -> &[PayeBand] {
        &self.bands
    }

    pub fn legal_effective_from(&self) -> NaiveDate {
        self.legal_effective_from
    }

    pub fn payroll_effective_from(&self) -> NaiveDate {
        self.payroll_applicability.from()
    }

    /// The complete interval this table applies to payroll for, per
    /// ADR-0007 — its `from` is `payroll_effective_from`, and it ends
    /// where the next `PayeTable` in the catalogue takes over, or is
    /// open-ended while this is the newest. `paye_table_for` selects on
    /// this alone; `legal_effective_from` is never consulted for
    /// selection.
    pub fn payroll_applicability(&self) -> EffectivePeriod {
        self.payroll_applicability
    }

    /// The exact, unrounded annual tax owed on `taxable` — statutory band
    /// arithmetic alone, with no `PayrollInput` or `YearToDateContext` in
    /// sight. Runs the same band walk as `PayrollRules::tax_owed_on`,
    /// entered here with a threshold scale factor of 1 (the full, unscaled
    /// annual bands), so a change to that per-period walk cannot silently
    /// diverge from this one.
    ///
    /// Never rounds. Rounding is Salt policy (SC-OPEN-2), decided by
    /// `RoundingRule`, not statutory arithmetic — a change to
    /// `RoundingRule` changes no `annual_tax` result. `taxable` stays
    /// `Money` at this boundary because taxable remuneration is a monetary
    /// domain value (non-negative, cents-exact); the result cannot be
    /// `Money`, since a cent above a threshold can owe a fraction of a
    /// cent in tax.
    pub fn annual_tax(&self, taxable: Money) -> Result<Decimal, PayrollError> {
        let (total, _) = band_walk(&self.bands, taxable.as_decimal(), |from| {
            Ok(from.as_decimal())
        })?;
        Ok(total)
    }
}

impl TryFrom<RawPayeTable> for PayeTable {
    type Error = PayrollRulesError;

    fn try_from(raw: RawPayeTable) -> Result<Self, PayrollRulesError> {
        PayeTable::new(
            raw.id,
            raw.bands,
            raw.legal_effective_from,
            raw.payroll_applicability,
        )
    }
}

impl From<PayeTable> for RawPayeTable {
    fn from(table: PayeTable) -> RawPayeTable {
        RawPayeTable {
            id: table.id,
            bands: table.bands,
            legal_effective_from: table.legal_effective_from,
            payroll_applicability: table.payroll_applicability,
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

/// Identifies an `SscRuleset`, independent of any `PayeTableId`: PAYE and
/// social security move on separate effective-date axes (ADR-0007).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SscRulesId(String);

impl SscRulesId {
    pub fn new(id: impl Into<String>) -> Self {
        SscRulesId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SscRulesId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The social security contribution rules in force for a payroll
/// applicability interval, with their own identity and their own pair of
/// effective dates (ADR-0007): `legal_effective_from` is what the
/// instrument says, `payroll_effective_from` — the start of
/// `payroll_applicability` — is what payroll actually applies.
/// `ssc_rules_for` resolves an `SscRuleset` on this axis alone, entirely
/// independent of `PayeTable` resolution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawSscRuleset", into = "RawSscRuleset")]
pub struct SscRuleset {
    id: SscRulesId,
    social_security: SocialSecurityRules,
    legal_effective_from: NaiveDate,
    payroll_applicability: EffectivePeriod,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawSscRuleset {
    pub id: SscRulesId,
    pub social_security: SocialSecurityRules,
    pub legal_effective_from: NaiveDate,
    pub payroll_applicability: EffectivePeriod,
}

impl SscRuleset {
    /// `payroll_applicability` must not start before
    /// `legal_effective_from` (ADR-0007: the two dates record a deferral,
    /// never a retro-active application).
    pub fn new(
        id: SscRulesId,
        employee_rate: Decimal,
        employer_rate: Decimal,
        floor: Money,
        ceiling: Money,
        legal_effective_from: NaiveDate,
        payroll_applicability: EffectivePeriod,
    ) -> Result<Self, PayrollRulesError> {
        SscRuleset::from_rules(
            id,
            SocialSecurityRules::new(employee_rate, employer_rate, floor, ceiling)?,
            legal_effective_from,
            payroll_applicability,
        )
    }

    /// The single validating constructor. `new` and the `serde` boundary
    /// both route through it, so an invariant added here cannot be
    /// bypassed by deserializing a `RawSscRuleset` — the way the previous
    /// hand-rolled `TryFrom` would have allowed.
    pub fn from_rules(
        id: SscRulesId,
        social_security: SocialSecurityRules,
        legal_effective_from: NaiveDate,
        payroll_applicability: EffectivePeriod,
    ) -> Result<Self, PayrollRulesError> {
        if payroll_applicability.from() < legal_effective_from {
            return Err(PayrollRulesError::PayrollAppliesBeforeLegalEffectiveDate);
        }
        Ok(SscRuleset {
            id,
            social_security,
            legal_effective_from,
            payroll_applicability,
        })
    }

    pub fn id(&self) -> &SscRulesId {
        &self.id
    }

    pub fn social_security(&self) -> SocialSecurityRules {
        self.social_security
    }

    pub fn legal_effective_from(&self) -> NaiveDate {
        self.legal_effective_from
    }

    pub fn payroll_effective_from(&self) -> NaiveDate {
        self.payroll_applicability.from()
    }

    /// The complete interval this ruleset applies to payroll for, per
    /// ADR-0007 — its `from` is `payroll_effective_from`, and it ends
    /// where the next `SscRuleset` in the catalogue takes over, or is
    /// open-ended while this is the newest. `ssc_rules_for` selects on
    /// this alone; `legal_effective_from` is never consulted for
    /// selection.
    pub fn payroll_applicability(&self) -> EffectivePeriod {
        self.payroll_applicability
    }
}

impl TryFrom<RawSscRuleset> for SscRuleset {
    type Error = PayrollRulesError;

    fn try_from(raw: RawSscRuleset) -> Result<Self, PayrollRulesError> {
        SscRuleset::from_rules(
            raw.id,
            raw.social_security,
            raw.legal_effective_from,
            raw.payroll_applicability,
        )
    }
}

impl From<SscRuleset> for RawSscRuleset {
    fn from(ruleset: SscRuleset) -> RawSscRuleset {
        RawSscRuleset {
            id: ruleset.id,
            social_security: ruleset.social_security,
            legal_effective_from: ruleset.legal_effective_from,
            payroll_applicability: ruleset.payroll_applicability,
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

/// The one progressive-band walk in the crate. Sums `rate * span` across
/// every band `taxable` reaches, where a band's threshold is
/// `scaled_threshold` applied to its `from`. `PayrollRules::tax_owed_on`
/// passes a closure that scales thresholds to `period_number`/12
/// (ADR-0001); `PayeTable::annual_tax` passes one that returns `from`
/// unscaled — a threshold scale factor of 1. Never a second copy of this
/// loop: that is what keeps a rounding-policy change from ever touching a
/// statutory result (SC-OPEN-2).
fn band_walk(
    bands: &[PayeBand],
    taxable: Decimal,
    scaled_threshold: impl Fn(Money) -> Result<Decimal, MoneyError>,
) -> Result<(Decimal, Vec<BandContribution>), MoneyError> {
    // Every step is checked: `Decimal`'s operators panic on overflow, and
    // a rules table is data a caller supplies.
    let mut total = Decimal::ZERO;
    let mut contributions = Vec::with_capacity(bands.len());
    for (index, band) in bands.iter().enumerate() {
        let threshold = scaled_threshold(band.from)?;
        if taxable <= threshold {
            break;
        }
        let upper = match bands.get(index + 1) {
            Some(next) => taxable.min(scaled_threshold(next.from)?),
            None => taxable,
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

/// The date range a `PayeTable` or `SscRuleset` is in force for, inclusive
/// at both ends — each axis's `payroll_applicability` (ADR-0007).
/// `paye_table_for` and `ssc_rules_for` each select on this range keyed by
/// `PayPeriod` end date (ADR-0005): a period straddling a change uses
/// whichever entry covers its end date, in full — statutory ceilings are
/// monthly amounts, never split pro-rata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawEffectivePeriod", into = "RawEffectivePeriod")]
pub struct EffectivePeriod {
    from: NaiveDate,
    /// The last date this entry covers, or `None` while it is the newest
    /// entry in force.
    until: Option<NaiveDate>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawEffectivePeriod {
    pub from: NaiveDate,
    pub until: Option<NaiveDate>,
}

impl EffectivePeriod {
    pub fn new(from: NaiveDate, until: Option<NaiveDate>) -> Result<Self, PayrollRulesError> {
        if until.is_some_and(|until| until < from) {
            return Err(PayrollRulesError::EffectivePeriodEndsBeforeItStarts);
        }
        Ok(EffectivePeriod { from, until })
    }

    pub fn from(self) -> NaiveDate {
        self.from
    }

    pub fn until(self) -> Option<NaiveDate> {
        self.until
    }

    /// Whether `date` falls within this range.
    pub fn covers(self, date: NaiveDate) -> bool {
        date >= self.from && self.until.is_none_or(|until| date <= until)
    }

    /// Whether this range shares any date with `other`. Two open-ended
    /// (`until: None`) ranges always overlap.
    pub fn overlaps(self, other: EffectivePeriod) -> bool {
        self.from <= other.until.unwrap_or(NaiveDate::MAX)
            && other.from <= self.until.unwrap_or(NaiveDate::MAX)
    }
}

impl TryFrom<RawEffectivePeriod> for EffectivePeriod {
    type Error = PayrollRulesError;

    fn try_from(raw: RawEffectivePeriod) -> Result<Self, PayrollRulesError> {
        EffectivePeriod::new(raw.from, raw.until)
    }
}

impl From<EffectivePeriod> for RawEffectivePeriod {
    fn from(period: EffectivePeriod) -> RawEffectivePeriod {
        RawEffectivePeriod {
            from: period.from,
            until: period.until,
        }
    }
}

/// The statutory and agreed calculation rules in force for a `PayPeriod`
/// end date: a `PayeTable` and an `SscRuleset`, each resolved on its own
/// effective-date axis and then frozen together by `ruleset_for`
/// (ADR-0007). Passed beside `PayrollInput`, never inside it, so a test
/// can vary rules against a fixed input (see `calculate`).
///
/// Carries no identity or effective period of its own: an id or a period
/// that changed whenever only one half moved would recreate the
/// combined-axis problem ADR-0007 removed. `calculate` checks the
/// `PayPeriod` end date against each half's own `payroll_applicability`
/// instead of a combined one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawPayrollRules", into = "RawPayrollRules")]
pub struct PayrollRules {
    paye_table: PayeTable,
    ssc_ruleset: SscRuleset,
    rounding_rule: RoundingRule,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawPayrollRules {
    pub paye_table: PayeTable,
    pub ssc_ruleset: SscRuleset,
    pub rounding_rule: RoundingRule,
}

impl PayrollRules {
    /// Infallible by construction: every invariant belongs to one of the
    /// three parts and has already been enforced by whoever built it —
    /// `PayeTable::new`, `SscRuleset::new`, and `RoundingRule` being a
    /// closed enum. `PayrollRules` deliberately adds no invariant of its
    /// own, because a cross-half invariant here would be a combined axis
    /// by another name (ADR-0007). A `Result` that can never be `Err`
    /// would only teach `ruleset_for` to write `.expect`.
    pub fn new(
        paye_table: PayeTable,
        ssc_ruleset: SscRuleset,
        rounding_rule: RoundingRule,
    ) -> Self {
        PayrollRules {
            paye_table,
            ssc_ruleset,
            rounding_rule,
        }
    }

    pub fn paye_table(&self) -> &PayeTable {
        &self.paye_table
    }

    pub fn ssc_ruleset(&self) -> &SscRuleset {
        &self.ssc_ruleset
    }

    pub fn social_security(&self) -> SocialSecurityRules {
        self.ssc_ruleset.social_security()
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
        let scaled_threshold = |from: Money| -> Result<Decimal, MoneyError> {
            from.as_decimal()
                .checked_mul(elapsed)
                .and_then(|scaled| scaled.checked_div(twelve))
                .ok_or(MoneyError::Overflow)
        };
        band_walk(&self.paye_table.bands, annual_taxable, scaled_threshold)
    }
}

impl From<RawPayrollRules> for PayrollRules {
    /// Total, unlike every other `Raw*` conversion in this module: the
    /// three parts each validate themselves on the way in through their
    /// own `serde` boundary, and `PayrollRules` adds nothing to check.
    fn from(raw: RawPayrollRules) -> PayrollRules {
        PayrollRules::new(raw.paye_table, raw.ssc_ruleset, raw.rounding_rule)
    }
}

impl From<PayrollRules> for RawPayrollRules {
    fn from(rules: PayrollRules) -> RawPayrollRules {
        RawPayrollRules {
            paye_table: rules.paye_table,
            ssc_ruleset: rules.ssc_ruleset,
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

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn test_applicability() -> EffectivePeriod {
        EffectivePeriod::new(date(2000, 1, 1), None).unwrap()
    }

    fn test_paye_table_id() -> PayeTableId {
        PayeTableId::new("test-paye-table")
    }

    fn paye_table() -> PayeTable {
        PayeTable::new(
            test_paye_table_id(),
            bands(),
            date(2000, 1, 1),
            test_applicability(),
        )
        .unwrap()
    }

    fn test_ssc_rules_id() -> SscRulesId {
        SscRulesId::new("test-ssc-rules")
    }

    fn ssc_ruleset() -> SscRuleset {
        SscRuleset::new(
            test_ssc_rules_id(),
            dec!(0.009),
            dec!(0.009),
            money(dec!(500)),
            money(dec!(11000)),
            date(2000, 1, 1),
            test_applicability(),
        )
        .unwrap()
    }

    fn rules() -> PayrollRules {
        PayrollRules::new(paye_table(), ssc_ruleset(), RoundingRule::HalfUpToCents)
    }

    #[test]
    fn algorithm_tax_owed_is_zero_within_the_first_band() {
        assert_eq!(rules().tax_owed_on(dec!(50000), 12).unwrap().0, dec!(0));
    }

    #[test]
    fn algorithm_tax_owed_is_zero_at_the_top_of_the_first_band() {
        assert_eq!(rules().tax_owed_on(dec!(120000), 12).unwrap().0, dec!(0));
    }

    #[test]
    fn algorithm_tax_owed_taxes_only_the_portion_within_the_second_band() {
        // At period 12 (full annual thresholds): 5,000 above the 120,000
        // threshold at 20%.
        assert_eq!(
            rules().tax_owed_on(dec!(125000), 12).unwrap().0,
            dec!(1000.00)
        );
    }

    #[test]
    fn algorithm_tax_owed_sums_across_every_band_crossed() {
        // 120,000 @ 0% + 120,000 @ 20% + 240,000 @ 30% + 30,000 @ 40%.
        assert_eq!(
            rules().tax_owed_on(dec!(510000), 12).unwrap().0,
            dec!(108000.00)
        );
    }

    #[test]
    fn algorithm_same_ytd_taxable_owes_more_later_in_the_tax_year() {
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
    fn algorithm_band_contributions_report_the_scaled_threshold_and_rate() {
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
    fn algorithm_annual_tax_matches_tax_owed_on_at_period_12() {
        // At period 12 the per-period walk's thresholds are the full,
        // unscaled annual bands — exactly what `annual_tax` runs.
        let (expected, _) = rules().tax_owed_on(dec!(510000), 12).unwrap();
        assert_eq!(
            paye_table().annual_tax(money(dec!(510000))).unwrap(),
            expected
        );
    }

    #[test]
    fn algorithm_annual_tax_is_zero_within_the_first_band() {
        assert_eq!(
            paye_table().annual_tax(money(dec!(50000))).unwrap(),
            dec!(0)
        );
    }

    #[test]
    fn algorithm_annual_tax_never_rounds() {
        // One cent above the second band's 120,000 threshold, at 20%, is
        // exactly N$0.002 — a sub-cent amount that no `Money` can hold.
        let tax = paye_table().annual_tax(money(dec!(120000.01))).unwrap();
        assert_eq!(tax, dec!(0.002));
        // And Salt's rounding policy would have destroyed it: applying
        // `RoundingRule` to that exact value yields N$0.00. The seam keeps
        // statutory arithmetic on the near side of that (SC-OPEN-2).
        assert_eq!(RoundingRule::HalfUpToCents.apply(tax).unwrap(), Money::ZERO);
    }

    #[test]
    fn algorithm_annual_tax_is_zero_at_the_top_of_the_first_band() {
        // The band boundary is exclusive at the bottom: 120,000 exactly is
        // still wholly inside the 0% band.
        assert_eq!(
            paye_table().annual_tax(money(dec!(120000))).unwrap(),
            dec!(0)
        );
    }

    #[test]
    fn algorithm_annual_tax_at_a_threshold_taxes_only_the_bands_below_it() {
        // 240,000 exactly: 120,000 @ 0% + 120,000 @ 20%, and nothing at
        // the 30% band it merely touches.
        assert_eq!(
            paye_table().annual_tax(money(dec!(240000))).unwrap(),
            dec!(24000.00)
        );
    }

    #[test]
    fn algorithm_annual_tax_sums_across_every_band_crossed() {
        // 120,000 @ 0% + 120,000 @ 20% + 240,000 @ 30% + 120,000 @ 40%.
        assert_eq!(
            paye_table().annual_tax(money(dec!(600000))).unwrap(),
            dec!(144000.00)
        );
    }

    #[test]
    fn algorithm_annual_tax_of_zero_is_zero() {
        assert_eq!(paye_table().annual_tax(Money::ZERO).unwrap(), dec!(0));
    }

    #[test]
    fn algorithm_annual_tax_refuses_when_band_arithmetic_overflows() {
        // A band table is data a caller supplies, so an absurd rate is
        // reachable: `PayeBand` only refuses a negative one. The largest
        // representable taxable amount at 1e12 exceeds `Decimal`, and the
        // checked step turns that into a refusal rather than a panic.
        let table = PayeTable::new(
            test_paye_table_id(),
            vec![PayeBand::new(Money::ZERO, dec!(1000000000000)).unwrap()],
            date(2000, 1, 1),
            test_applicability(),
        )
        .unwrap();
        assert_eq!(
            table.annual_tax(Money::from_cents(i64::MAX).unwrap()),
            Err(PayrollError::AmountOverflow)
        );
    }

    #[test]
    fn algorithm_annual_tax_and_tax_owed_on_agree_across_the_whole_table() {
        // The shared walk, checked at every band boundary and either side
        // of it: one implementation cannot drift from itself.
        for amount in [
            dec!(0),
            dec!(0.01),
            dec!(119999.99),
            dec!(120000),
            dec!(120000.01),
            dec!(239999.99),
            dec!(240000),
            dec!(240000.01),
            dec!(479999.99),
            dec!(480000),
            dec!(480000.01),
            dec!(1000000),
        ] {
            let (per_period, _) = rules().tax_owed_on(amount, 12).unwrap();
            assert_eq!(
                paye_table().annual_tax(money(amount)).unwrap(),
                per_period,
                "annual_tax disagreed with tax_owed_on at {amount}"
            );
        }
    }

    #[test]
    fn algorithm_annual_tax_reads_the_same_table_the_rules_carry() {
        let rules = rules();
        assert_eq!(
            rules.paye_table().annual_tax(money(dec!(510000))).unwrap(),
            paye_table().annual_tax(money(dec!(510000))).unwrap()
        );
        assert_eq!(rules.paye_table().id(), &test_paye_table_id());
    }

    #[test]
    fn paye_table_keeps_both_of_its_effective_dates() {
        // The two dates are distinct facts (ADR-0007): what the instrument
        // says, and what payroll actually applied.
        let table = PayeTable::new(
            test_paye_table_id(),
            bands(),
            date(2026, 3, 1),
            EffectivePeriod::new(date(2026, 9, 1), None).unwrap(),
        )
        .unwrap();
        assert_eq!(table.legal_effective_from(), date(2026, 3, 1));
        assert_eq!(table.payroll_effective_from(), date(2026, 9, 1));
        let json = serde_json::to_string(&table).unwrap();
        assert_eq!(serde_json::from_str::<PayeTable>(&json).unwrap(), table);
    }

    #[test]
    fn deserialize_rejects_an_empty_band_table() {
        let json = serde_json::to_string(&RawPayeTable {
            id: test_paye_table_id(),
            bands: Vec::new(),
            legal_effective_from: date(2000, 1, 1),
            payroll_applicability: test_applicability(),
        })
        .unwrap();
        assert!(serde_json::from_str::<PayeTable>(&json).is_err());
    }

    #[test]
    fn deserialize_rejects_a_first_band_that_does_not_start_at_zero() {
        let json = serde_json::to_string(&RawPayeTable {
            id: test_paye_table_id(),
            bands: vec![PayeBand::new(money(dec!(100)), dec!(0.2)).unwrap()],
            legal_effective_from: date(2000, 1, 1),
            payroll_applicability: test_applicability(),
        })
        .unwrap();
        assert!(serde_json::from_str::<PayeTable>(&json).is_err());
    }

    #[test]
    fn algorithm_base_clamps_to_the_ceiling() {
        assert_eq!(
            social_security().base(money(dec!(20000))),
            (money(dec!(11000)), SscClamp::Ceiling)
        );
    }

    #[test]
    fn algorithm_base_clamps_to_the_floor() {
        assert_eq!(
            social_security().base(money(dec!(300))),
            (money(dec!(500)), SscClamp::Floor)
        );
    }

    #[test]
    fn algorithm_base_is_unchanged_between_the_floor_and_ceiling() {
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
            PayeTable::new(
                test_paye_table_id(),
                Vec::new(),
                date(2000, 1, 1),
                test_applicability()
            ),
            Err(PayrollRulesError::EmptyBandTable)
        );
    }

    #[test]
    fn rejects_a_first_band_that_does_not_start_at_zero() {
        let bands = vec![PayeBand::new(money(dec!(100)), dec!(0.20)).unwrap()];
        assert_eq!(
            PayeTable::new(
                test_paye_table_id(),
                bands,
                date(2000, 1, 1),
                test_applicability()
            ),
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
            PayeTable::new(
                test_paye_table_id(),
                bands,
                date(2000, 1, 1),
                test_applicability()
            ),
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
    fn paye_table_deserialize_round_trips() {
        let table = paye_table();
        let json = serde_json::to_string(&table).unwrap();
        assert_eq!(serde_json::from_str::<PayeTable>(&json).unwrap(), table);
    }

    #[test]
    fn deserialize_rejects_an_unsorted_band_table() {
        let json = serde_json::to_string(&RawPayeTable {
            id: test_paye_table_id(),
            bands: vec![
                PayeBand::new(money(dec!(0)), dec!(0.0)).unwrap(),
                PayeBand::new(money(dec!(200)), dec!(0.1)).unwrap(),
                PayeBand::new(money(dec!(100)), dec!(0.2)).unwrap(),
            ],
            legal_effective_from: date(2000, 1, 1),
            payroll_applicability: test_applicability(),
        })
        .unwrap();
        assert!(serde_json::from_str::<PayeTable>(&json).is_err());
    }

    #[test]
    fn effective_period_covers_from_onward_when_open_ended() {
        let period = EffectivePeriod::new(date(2026, 9, 1), None).unwrap();
        assert!(!period.covers(date(2026, 8, 31)));
        assert!(period.covers(date(2026, 9, 1)));
        assert!(period.covers(date(2030, 1, 1)));
    }

    #[test]
    fn effective_period_covers_only_up_to_until_inclusive() {
        let period = EffectivePeriod::new(date(2025, 3, 1), Some(date(2026, 8, 31))).unwrap();
        assert!(period.covers(date(2025, 3, 1)));
        assert!(period.covers(date(2026, 8, 31)));
        assert!(!period.covers(date(2026, 9, 1)));
    }

    #[test]
    fn rejects_an_effective_period_that_ends_before_it_starts() {
        assert_eq!(
            EffectivePeriod::new(date(2026, 9, 1), Some(date(2026, 8, 31))),
            Err(PayrollRulesError::EffectivePeriodEndsBeforeItStarts)
        );
    }

    #[test]
    fn adjacent_effective_periods_do_not_overlap() {
        let earlier = EffectivePeriod::new(date(2025, 3, 1), Some(date(2026, 8, 31))).unwrap();
        let later = EffectivePeriod::new(date(2026, 9, 1), None).unwrap();
        assert!(!earlier.overlaps(later));
        assert!(!later.overlaps(earlier));
    }

    #[test]
    fn effective_periods_sharing_a_date_overlap() {
        let earlier = EffectivePeriod::new(date(2025, 3, 1), Some(date(2026, 9, 1))).unwrap();
        let later = EffectivePeriod::new(date(2026, 9, 1), None).unwrap();
        assert!(earlier.overlaps(later));
        assert!(later.overlaps(earlier));
    }

    #[test]
    fn effective_period_deserialize_round_trips() {
        let period = EffectivePeriod::new(date(2025, 3, 1), Some(date(2026, 8, 31))).unwrap();
        let json = serde_json::to_string(&period).unwrap();
        assert_eq!(
            serde_json::from_str::<EffectivePeriod>(&json).unwrap(),
            period
        );
    }

    #[test]
    fn ssc_ruleset_keeps_both_of_its_effective_dates() {
        // Mirrors `paye_table_keeps_both_of_its_effective_dates`: the two
        // dates are distinct facts (ADR-0007), and an `SscRuleset` carries
        // them exactly like a `PayeTable` does.
        let ruleset = SscRuleset::new(
            test_ssc_rules_id(),
            dec!(0.009),
            dec!(0.009),
            money(dec!(500)),
            money(dec!(12500)),
            date(2026, 3, 1),
            EffectivePeriod::new(date(2026, 9, 1), None).unwrap(),
        )
        .unwrap();
        assert_eq!(ruleset.legal_effective_from(), date(2026, 3, 1));
        assert_eq!(ruleset.payroll_effective_from(), date(2026, 9, 1));
        assert_eq!(ruleset.id(), &test_ssc_rules_id());
    }

    #[test]
    fn ssc_ruleset_new_delegates_negative_rate_validation() {
        assert_eq!(
            SscRuleset::new(
                test_ssc_rules_id(),
                dec!(-0.01),
                dec!(0.009),
                money(dec!(500)),
                money(dec!(11000)),
                date(2000, 1, 1),
                test_applicability(),
            ),
            Err(PayrollRulesError::NegativeRate)
        );
    }

    #[test]
    fn ssc_ruleset_new_delegates_floor_above_ceiling_validation() {
        assert_eq!(
            SscRuleset::new(
                test_ssc_rules_id(),
                dec!(0.009),
                dec!(0.009),
                money(dec!(11000)),
                money(dec!(500)),
                date(2000, 1, 1),
                test_applicability(),
            ),
            Err(PayrollRulesError::FloorAboveCeiling)
        );
    }

    #[test]
    fn ssc_ruleset_deserialize_round_trips() {
        let ruleset = ssc_ruleset();
        let json = serde_json::to_string(&ruleset).unwrap();
        assert_eq!(serde_json::from_str::<SscRuleset>(&json).unwrap(), ruleset);
    }

    // ADR-0007's two dates record a deferral: `ssc-2026-09` is gazetted
    // 1 March 2026 but applied from the September 2026 payroll. The
    // reverse — payroll applying a rule before the instrument gives it
    // legal effect — would withhold under a rule that did not yet exist,
    // so it is refused at construction on both axes.
    #[test]
    fn paye_table_refuses_payroll_applicability_starting_before_its_legal_effective_date() {
        assert_eq!(
            PayeTable::new(
                test_paye_table_id(),
                bands(),
                date(2026, 3, 1),
                EffectivePeriod::new(date(2026, 2, 28), None).unwrap(),
            ),
            Err(PayrollRulesError::PayrollAppliesBeforeLegalEffectiveDate)
        );
    }

    #[test]
    fn ssc_ruleset_refuses_payroll_applicability_starting_before_its_legal_effective_date() {
        assert_eq!(
            SscRuleset::new(
                test_ssc_rules_id(),
                dec!(0.009),
                dec!(0.009),
                money(dec!(500)),
                money(dec!(11000)),
                date(2026, 3, 1),
                EffectivePeriod::new(date(2026, 2, 28), None).unwrap(),
            ),
            Err(PayrollRulesError::PayrollAppliesBeforeLegalEffectiveDate)
        );
    }

    #[test]
    fn a_deferred_payroll_effective_date_is_accepted_on_both_axes() {
        let deferred = EffectivePeriod::new(date(2026, 9, 1), None).unwrap();
        assert!(PayeTable::new(test_paye_table_id(), bands(), date(2026, 3, 1), deferred).is_ok());
        assert!(
            SscRuleset::new(
                test_ssc_rules_id(),
                dec!(0.009),
                dec!(0.009),
                money(dec!(500)),
                money(dec!(11000)),
                date(2026, 3, 1),
                deferred,
            )
            .is_ok()
        );
    }

    // `SscRuleset` is the one type here whose `Raw` form once bypassed its
    // constructor. Deserializing must run the same validation, or an
    // invariant added to `from_rules` would hold for `new` and silently
    // not for `serde`.
    #[test]
    fn ssc_ruleset_deserialize_rejects_payroll_applicability_before_the_legal_date() {
        let json = serde_json::json!({
            "id": "ssc-backdated",
            "social_security": {
                "employee_rate": "0.009",
                "employer_rate": "0.009",
                "floor": 50000,
                "ceiling": 1100000
            },
            "legal_effective_from": "2026-03-01",
            "payroll_applicability": { "from": "2026-02-28", "until": null }
        })
        .to_string();

        assert!(
            serde_json::from_str::<SscRuleset>(&json)
                .unwrap_err()
                .to_string()
                .contains("payroll applicability starts before the legal effective date")
        );

        // Positive control: the identical document with the legal date
        // moved back parses, proving the refusal above is the invariant
        // firing and not a malformed fixture.
        assert!(
            serde_json::from_str::<SscRuleset>(&json.replace("2026-03-01", "2026-01-01")).is_ok()
        );
    }

    #[test]
    fn payroll_rules_deserialize_round_trips() {
        let rules = rules();
        let json = serde_json::to_string(&rules).unwrap();
        assert_eq!(serde_json::from_str::<PayrollRules>(&json).unwrap(), rules);
    }
}
