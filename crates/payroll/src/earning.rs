//! `Earning`: one classified line of money owed for the period, and the
//! three statutory bases those lines feed.

use serde::{Deserialize, Serialize, de::Error as DeError};

use crate::money::{Money, MoneyError};

/// The maximum number of Unicode scalar values an `EarningLabel` may carry.
/// Labels are for humans, so the bound protects storage and display without
/// imposing an arbitrary byte limit on non-ASCII text.
pub const MAX_EARNING_LABEL_LENGTH: usize = 100;

/// Why an `EarningLabel` could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EarningLabelError {
    /// The label was empty or contained only whitespace.
    Empty,
    /// The label exceeded [`MAX_EARNING_LABEL_LENGTH`].
    TooLong { maximum: usize },
}

impl std::fmt::Display for EarningLabelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EarningLabelError::Empty => write!(f, "earning label must not be empty"),
            EarningLabelError::TooLong { maximum } => {
                write!(f, "earning label must be at most {maximum} characters")
            }
        }
    }
}

impl std::error::Error for EarningLabelError {}

/// Human-facing text attached to a classified earning line.
///
/// The label is deliberately a separate type from the earning classification:
/// it is shown to a person and is never read by the calculator's arithmetic.
/// Construction trims surrounding whitespace and enforces the non-blank and
/// length invariants, including when a value is deserialized from history.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EarningLabel(String);

impl EarningLabel {
    pub fn new(label: impl Into<String>) -> Result<Self, EarningLabelError> {
        let label = label.into();
        let trimmed = label.trim();
        if trimmed.is_empty() {
            return Err(EarningLabelError::Empty);
        }
        if trimmed.chars().count() > MAX_EARNING_LABEL_LENGTH {
            return Err(EarningLabelError::TooLong {
                maximum: MAX_EARNING_LABEL_LENGTH,
            });
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for EarningLabel {
    type Error = EarningLabelError;

    fn try_from(label: String) -> Result<Self, Self::Error> {
        Self::new(label)
    }
}

impl From<EarningLabel> for String {
    fn from(label: EarningLabel) -> String {
        label.0
    }
}

impl std::fmt::Display for EarningLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One earning instruction supplied to the calculator. `BasicPay` is absent
/// by construction: it is derived from the Employment's CompensationTerms,
/// so an input can never express a duplicate BasicPay line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum EarningInstruction {
    /// An allowance that counts toward `TaxableRemuneration`.
    ///
    /// `None` exists only for reading version-1 history, whose bare allowance
    /// had no label. New HTTP requests are required to provide `Some`.
    TaxableAllowance {
        amount: Money,
        label: Option<EarningLabel>,
    },
}

impl EarningInstruction {
    pub fn amount(&self) -> Money {
        match self {
            Self::TaxableAllowance { amount, .. } => *amount,
        }
    }

    pub fn label(&self) -> Option<&EarningLabel> {
        match self {
            Self::TaxableAllowance { label, .. } => label.as_ref(),
        }
    }
}

/// One classified line of money owed to the employee for the period.
/// Classification, not description, decides tax treatment: there is no
/// separate `taxable: bool` flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Earning {
    /// The contractual base amount for the period. The base for social
    /// security contributions.
    BasicPay(Money),
    /// An allowance that counts toward `TaxableRemuneration`.
    TaxableAllowance {
        amount: Money,
        /// `None` is retained only when a version-1 snapshot had an
        /// unlabelled allowance. Current HTTP-created lines always carry a
        /// label, but history must not be given an invented one.
        label: Option<EarningLabel>,
    },
}

impl Earning {
    pub fn amount(&self) -> Money {
        match self {
            Earning::BasicPay(amount) => *amount,
            Earning::TaxableAllowance { amount, .. } => *amount,
        }
    }
}

/// The current and version-1 shapes of a taxable allowance. Version 1 used
/// the bare `Money` form; the current shape carries the optional label.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawTaxableAllowance {
    Current {
        amount: Money,
        label: Option<EarningLabel>,
    },
    Legacy(Money),
}

impl RawTaxableAllowance {
    fn into_parts(self) -> (Money, Option<EarningLabel>) {
        match self {
            Self::Current { amount, label } => (amount, label),
            Self::Legacy(amount) => (amount, None),
        }
    }
}

#[derive(Debug, Deserialize)]
enum RawEarning {
    BasicPay(Money),
    TaxableAllowance(RawTaxableAllowance),
}

impl<'de> Deserialize<'de> for Earning {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match RawEarning::deserialize(deserializer)? {
            RawEarning::BasicPay(amount) => Ok(Self::BasicPay(amount)),
            RawEarning::TaxableAllowance(raw) => {
                let (amount, label) = raw.into_parts();
                Ok(Self::TaxableAllowance { amount, label })
            }
        }
    }
}

impl<'de> Deserialize<'de> for EarningInstruction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match RawEarning::deserialize(deserializer)? {
            RawEarning::BasicPay(_) => Err(D::Error::custom(
                "BasicPay cannot be supplied as an earning instruction",
            )),
            RawEarning::TaxableAllowance(raw) => {
                let (amount, label) = raw.into_parts();
                Ok(Self::TaxableAllowance { amount, label })
            }
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
///
/// `gross` and `taxable` carry equal amounts today, because every v1
/// earning kind feeds both. That is an arithmetic coincidence of the
/// current two kinds, not an identity: gross is everything payable and
/// taxable is what PAYE is charged on, and they are different questions.
/// Do not collapse the two fields. A legally-named kind added later — a
/// qualifying subsistence or business-travel reimbursement, which
/// Schedule 2 excludes from remuneration — feeds gross without feeding
/// taxable, and the field it would need must already be here.
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
            // The two-way table of `docs/domain/payroll-calculation.md`
            // §5.2, stated once. `BasicPay` counts toward SSC, PAYE, and
            // gross; `TaxableAllowance` toward PAYE and gross but not
            // SSC. Each line states which of the three
            // bases it feeds in this one exhaustive match, so a future
            // kind cannot be added without deciding its effect on all
            // three here.
            match line {
                Earning::BasicPay(amount) => {
                    bases.social_security = bases.social_security.checked_add(*amount)?;
                    bases.taxable = bases.taxable.checked_add(*amount)?;
                    bases.gross = bases.gross.checked_add(*amount)?;
                }
                Earning::TaxableAllowance { amount, .. } => {
                    bases.taxable = bases.taxable.checked_add(*amount)?;
                    bases.gross = bases.gross.checked_add(*amount)?;
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

    /// `GrossRemuneration` — everything payable, whatever its tax
    /// treatment. Its own accumulator, never derived from `taxable`.
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
        let bases = RemunerationBases::accumulate(&[Earning::TaxableAllowance {
            amount: money(dec!(2000.00)),
            label: None,
        }])
        .unwrap();

        assert_eq!(bases.social_security(), Money::ZERO);
        assert_eq!(bases.taxable(), money(dec!(2000.00)));
        assert_eq!(bases.gross(), money(dec!(2000.00)));
    }

    #[test]
    fn social_security_differs_from_taxable_and_gross_when_an_allowance_is_present() {
        let bases = RemunerationBases::accumulate(&[
            Earning::BasicPay(money(dec!(15000.00))),
            Earning::TaxableAllowance {
                amount: money(dec!(2000.00)),
                label: None,
            },
        ])
        .unwrap();

        assert_eq!(bases.social_security(), money(dec!(15000.00)));
        assert_eq!(bases.taxable(), money(dec!(17000.00)));
        assert_eq!(bases.gross(), money(dec!(17000.00)));
    }

    #[test]
    fn repeated_lines_of_one_kind_each_count_once() {
        let bases = RemunerationBases::accumulate(&[
            Earning::TaxableAllowance {
                amount: money(dec!(250.00)),
                label: None,
            },
            Earning::TaxableAllowance {
                amount: money(dec!(250.00)),
                label: None,
            },
        ])
        .unwrap();

        assert_eq!(bases.taxable(), money(dec!(500.00)));
        assert_eq!(bases.gross(), money(dec!(500.00)));
    }

    #[test]
    fn an_overflowing_total_is_reported_rather_than_wrapped() {
        let huge = Money::from_cents(i64::MAX).unwrap();
        let lines = [
            Earning::TaxableAllowance {
                amount: huge,
                label: None,
            },
            Earning::BasicPay(huge),
        ];

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
            Earning::TaxableAllowance {
                amount: money(dec!(2.00)),
                label: None,
            }
            .amount(),
            money(dec!(2.00))
        );
    }

    #[test]
    fn deserialize_round_trips() {
        let line = Earning::TaxableAllowance {
            amount: money(dec!(2000.00)),
            label: Some(EarningLabel::new("standby allowance").unwrap()),
        };
        let json = serde_json::to_string(&line).unwrap();

        assert_eq!(serde_json::from_str::<Earning>(&json).unwrap(), line);
    }

    #[test]
    fn an_earning_label_is_trimmed() {
        let label = EarningLabel::new("  standby allowance  ").unwrap();

        assert_eq!(label.as_str(), "standby allowance");
    }

    #[test]
    fn an_earning_label_must_not_be_blank() {
        assert_eq!(EarningLabel::new("  "), Err(EarningLabelError::Empty));
    }

    #[test]
    fn an_earning_label_is_bounded() {
        let too_long = "x".repeat(MAX_EARNING_LABEL_LENGTH + 1);

        assert_eq!(
            EarningLabel::new(too_long),
            Err(EarningLabelError::TooLong {
                maximum: MAX_EARNING_LABEL_LENGTH
            })
        );
    }

    #[test]
    fn a_version_one_allowance_reads_back_without_an_invented_label() {
        let old_json = serde_json::json!({
            "TaxableAllowance": 200000
        });

        let instruction: EarningInstruction = serde_json::from_value(old_json.clone()).unwrap();
        let earning: Earning = serde_json::from_value(old_json).unwrap();

        assert_eq!(
            instruction,
            EarningInstruction::TaxableAllowance {
                amount: money(dec!(2000.00)),
                label: None,
            }
        );
        assert_eq!(
            earning,
            Earning::TaxableAllowance {
                amount: money(dec!(2000.00)),
                label: None,
            }
        );
    }
}
