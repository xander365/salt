//! `Earning`: one classified line of money owed for the period, and the
//! three statutory bases those lines feed.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize, de::Error as DeError};

use crate::employment::OrdinaryHours;
use crate::money::{Money, MoneyError};
use crate::salt_policy::SaltPolicyStamp;

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

/// The largest number of overtime hours one line may carry. A pay period is
/// at most 31 days, so 744 is every hour in the longest period there is:
/// past that the figure is a typing mistake, not overtime. The bound exists
/// to keep a slipped decimal point out of the payroll rather than to state
/// any rule about working time — the Labour Act's own overtime limits are
/// the T&A system's business, not Salt's (D14).
pub const MAX_OVERTIME_HOURS: Decimal = Decimal::from_parts(744, 0, 0, false, 0);

/// Why an [`OvertimeHours`] value cannot be recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OvertimeHoursError {
    /// Zero or fewer hours. Zero overtime is expressed by having no
    /// overtime line at all, never by a line claiming nothing happened.
    ZeroOrNegative,
    /// More than [`MAX_OVERTIME_HOURS`] on one line.
    MoreThanAWholePeriod { maximum: Decimal },
    /// Finer than a hundredth of an hour.
    MoreThanTwoDecimalPlaces,
}

impl std::fmt::Display for OvertimeHoursError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroOrNegative => write!(f, "overtime hours must be greater than zero"),
            Self::MoreThanAWholePeriod { maximum } => {
                write!(f, "overtime hours must not exceed {maximum} on one line")
            }
            Self::MoreThanTwoDecimalPlaces => {
                write!(
                    f,
                    "overtime hours must have no more than two decimal places"
                )
            }
        }
    }
}

impl std::error::Error for OvertimeHoursError {}

/// Hours actually worked at one overtime multiplier in one PayPeriod.
///
/// A distinct type from [`OrdinaryHours`], and deliberately not
/// interchangeable with it: `OrdinaryHours` is a contractual weekly
/// assumption used to derive a rate, while this is hours a person worked
/// and is being paid for. Confusing the two would price overtime off the
/// wrong number, so the compiler is made to keep them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "Decimal", into = "Decimal")]
pub struct OvertimeHours(Decimal);

impl OvertimeHours {
    pub fn new(hours: Decimal) -> Result<Self, OvertimeHoursError> {
        if hours <= Decimal::ZERO {
            return Err(OvertimeHoursError::ZeroOrNegative);
        }
        if hours > MAX_OVERTIME_HOURS {
            return Err(OvertimeHoursError::MoreThanAWholePeriod {
                maximum: MAX_OVERTIME_HOURS,
            });
        }
        if hours.scale() > 2 {
            return Err(OvertimeHoursError::MoreThanTwoDecimalPlaces);
        }
        Ok(Self(hours))
    }

    pub fn as_decimal(self) -> Decimal {
        self.0
    }
}

impl TryFrom<Decimal> for OvertimeHours {
    type Error = OvertimeHoursError;

    fn try_from(hours: Decimal) -> Result<Self, Self::Error> {
        Self::new(hours)
    }
}

impl From<OvertimeHours> for Decimal {
    fn from(hours: OvertimeHours) -> Decimal {
        hours.0
    }
}

/// Why an [`OvertimeMultiplier`] could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OvertimeMultiplierError {
    /// A factor outside the closed set. Carries what was supplied so the
    /// refusal can say which number was refused, not merely that one was.
    Unsupported { supplied: Decimal },
}

impl std::fmt::Display for OvertimeMultiplierError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported { supplied } => write!(
                f,
                "overtime multiplier {supplied} is not supported; Salt supports 1.5 and 2 only"
            ),
        }
    }
}

impl std::error::Error for OvertimeMultiplierError {}

/// The overtime factor, which **is** the classification (D19). An `enum`
/// and not a `Decimal` field, because the set is closed at 1.5 and 2.0
/// (D31): a third factor is a code change and a deliberate decision,
/// exactly as a third [`Earning`] kind is, and must never become a
/// configuration value an Operator can type.
///
/// Whether 1.5 and 2.0 are the correct and only statutory factors in
/// Namibia is `Q-OPEN-8` — assumed from the owner's practice and not
/// verified from any statute. Nothing here may present them as law.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "Decimal", into = "Decimal")]
pub enum OvertimeMultiplier {
    /// Time and a half.
    OneAndAHalf,
    /// Double time.
    Double,
}

impl OvertimeMultiplier {
    pub fn as_decimal(self) -> Decimal {
        match self {
            OvertimeMultiplier::OneAndAHalf => Decimal::from_parts(15, 0, 0, false, 1),
            OvertimeMultiplier::Double => Decimal::from_parts(2, 0, 0, false, 0),
        }
    }
}

impl TryFrom<Decimal> for OvertimeMultiplier {
    type Error = OvertimeMultiplierError;

    /// Compared by value, not by representation: `1.50`, `1.5` and `1.500`
    /// are the same factor, and a trailing zero an Operator typed must not
    /// decide whether their overtime is accepted.
    fn try_from(multiplier: Decimal) -> Result<Self, Self::Error> {
        if multiplier == OvertimeMultiplier::OneAndAHalf.as_decimal() {
            Ok(OvertimeMultiplier::OneAndAHalf)
        } else if multiplier == OvertimeMultiplier::Double.as_decimal() {
            Ok(OvertimeMultiplier::Double)
        } else {
            Err(OvertimeMultiplierError::Unsupported {
                supplied: multiplier,
            })
        }
    }
}

impl From<OvertimeMultiplier> for Decimal {
    fn from(multiplier: OvertimeMultiplier) -> Decimal {
        multiplier.as_decimal()
    }
}

/// Every figure behind one overtime line, so an Operator can check the
/// money by hand. Structured data, like [`crate::PayeTrace`] and
/// [`crate::SscTrace`] — no free-text formula strings, and the
/// `SaltPolicy` mark is carried as a [`SaltPolicyStamp`] so the screen
/// renders the sentence rather than the calculator emitting one.
///
/// `basic_pay` is the **contractual** figure from `CompensationTerms`,
/// never the prorated one: a person's hourly rate does not fall because
/// they joined mid-month. That choice is part of SC-OPEN-6 and is stamped
/// with it.
///
/// `derived_hourly_rate` is exact and unrounded, like
/// `PayeTrace::year_to_date_tax_owed`. There is exactly one rounding on an
/// overtime line and it happens at the line, through the Salt rounding
/// policy — never on the rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvertimeTrace {
    pub basic_pay: Money,
    pub ordinary_hours: OrdinaryHours,
    /// The `12` in `BasicPay x 12 / 52 / OrdinaryHours`. Carried as data,
    /// not as a literal in a sentence, so the workings show the divisor
    /// the figure was actually derived with.
    pub months_per_year: Decimal,
    /// The `52`.
    pub weeks_per_year: Decimal,
    pub derived_hourly_rate: Decimal,
    pub hours: OvertimeHours,
    pub multiplier: OvertimeMultiplier,
    /// SC-OPEN-6, `NEEDS CONFIRMATION`. Salt chose this divisor; no
    /// regulator prescribed it.
    pub policy: SaltPolicyStamp,
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
    /// Hours at a fixed multiplier (D14, ADR-0022). No amount: Salt derives
    /// the money from the Employment's own `BasicPay` and `OrdinaryHours`,
    /// which is the whole point of typing hours rather than a figure worked
    /// out in a spreadsheet.
    Overtime {
        hours: OvertimeHours,
        multiplier: OvertimeMultiplier,
        label: Option<EarningLabel>,
    },
}

impl EarningInstruction {
    /// There is deliberately no `amount()` here. An overtime instruction
    /// carries hours, not money, so no total function from an instruction
    /// to a `Money` exists — the money appears only on the calculated
    /// [`Earning`] line.
    pub fn label(&self) -> Option<&EarningLabel> {
        match self {
            Self::TaxableAllowance { label, .. } | Self::Overtime { label, .. } => label.as_ref(),
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
    /// Overtime, already priced. `amount` is the one rounded figure on the
    /// line; `trace` holds every input it was derived from, including the
    /// unrounded hourly rate and the `SaltPolicy` stamp on the divisor.
    Overtime {
        amount: Money,
        trace: OvertimeTrace,
        /// The free-text reason ("Sunday overtime"). Never read by any
        /// arithmetic: the multiplier is the classification, not this.
        label: Option<EarningLabel>,
    },
}

impl Earning {
    pub fn amount(&self) -> Money {
        match self {
            Earning::BasicPay(amount) => *amount,
            Earning::TaxableAllowance { amount, .. } => *amount,
            Earning::Overtime { amount, .. } => *amount,
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

/// The wire shape of a calculated [`Earning`]. Separate from
/// [`RawEarningInstruction`] below because the two genuinely differ now:
/// an overtime *line* carries money and workings, an overtime *instruction*
/// carries hours. One shared raw type would have to make every field
/// optional and could no longer refuse either half's nonsense.
#[derive(Debug, Deserialize)]
enum RawEarning {
    BasicPay(Money),
    TaxableAllowance(RawTaxableAllowance),
    Overtime {
        amount: Money,
        trace: OvertimeTrace,
        label: Option<EarningLabel>,
    },
}

/// The wire shape of an [`EarningInstruction`]. `BasicPay` appears here
/// only so it can be refused by name rather than as an unknown variant, and
/// its payload is ignored: the refusal is on the *kind*, so it must not
/// depend on the amount beside it being well formed.
#[derive(Debug, Deserialize)]
enum RawEarningInstruction {
    BasicPay(serde::de::IgnoredAny),
    TaxableAllowance(RawTaxableAllowance),
    Overtime {
        hours: OvertimeHours,
        multiplier: OvertimeMultiplier,
        label: Option<EarningLabel>,
    },
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
            RawEarning::Overtime {
                amount,
                trace,
                label,
            } => Ok(Self::Overtime {
                amount,
                trace,
                label,
            }),
        }
    }
}

impl<'de> Deserialize<'de> for EarningInstruction {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match RawEarningInstruction::deserialize(deserializer)? {
            RawEarningInstruction::BasicPay(_) => Err(D::Error::custom(
                "BasicPay cannot be supplied as an earning instruction",
            )),
            RawEarningInstruction::TaxableAllowance(raw) => {
                let (amount, label) = raw.into_parts();
                Ok(Self::TaxableAllowance { amount, label })
            }
            RawEarningInstruction::Overtime {
                hours,
                multiplier,
                label,
            } => Ok(Self::Overtime {
                hours,
                multiplier,
                label,
            }),
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
                // Overtime's exclusion from the social security base is
                // settled law, not a Salt choice: the Social Security
                // General Regulations define `basic wage` as remuneration
                // for ordinary work and exclude overtime from it
                // (`docs/domain/statutory-conformance.md` §3.3). It feeds
                // PAYE and gross like any other taxable pay.
                Earning::Overtime { amount, .. } => {
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

    /// Workings for one overtime line, so the accumulator and the
    /// serialization tests have a whole `Earning::Overtime` to work with.
    /// The figures are the running example: N$12,000 over 40 weekly hours.
    fn a_trace() -> OvertimeTrace {
        OvertimeTrace {
            basic_pay: money(dec!(12000.00)),
            ordinary_hours: OrdinaryHours::new(dec!(40)).unwrap(),
            months_per_year: dec!(12),
            weeks_per_year: dec!(52),
            derived_hourly_rate: dec!(12000.00) * dec!(12) / dec!(52) / dec!(40),
            hours: OvertimeHours::new(dec!(12)).unwrap(),
            multiplier: OvertimeMultiplier::OneAndAHalf,
            policy: SaltPolicyStamp::SALARY_TO_HOURLY_DIVISOR,
        }
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

    // ---- Overtime (issue #76, ADR-0022) -----------------------------------

    #[test]
    fn an_overtime_line_feeds_paye_and_gross_but_never_the_social_security_base() {
        let bases = RemunerationBases::accumulate(&[Earning::Overtime {
            amount: money(dec!(1246.15)),
            trace: a_trace(),
            label: None,
        }])
        .unwrap();

        assert_eq!(bases.social_security(), Money::ZERO);
        assert_eq!(bases.taxable(), money(dec!(1246.15)));
        assert_eq!(bases.gross(), money(dec!(1246.15)));
    }

    #[test]
    fn a_multiplier_outside_the_closed_set_is_refused_with_a_stated_reason() {
        let refusal = OvertimeMultiplier::try_from(dec!(1.75)).unwrap_err();

        assert_eq!(
            refusal,
            OvertimeMultiplierError::Unsupported {
                supplied: dec!(1.75)
            }
        );
        assert_eq!(
            refusal.to_string(),
            "overtime multiplier 1.75 is not supported; Salt supports 1.5 and 2 only"
        );
    }

    #[test]
    fn the_two_supported_multipliers_are_accepted_however_they_are_written() {
        for written in [dec!(1.5), dec!(1.50), dec!(1.500)] {
            assert_eq!(
                OvertimeMultiplier::try_from(written),
                Ok(OvertimeMultiplier::OneAndAHalf)
            );
        }
        for written in [dec!(2), dec!(2.0), dec!(2.00)] {
            assert_eq!(
                OvertimeMultiplier::try_from(written),
                Ok(OvertimeMultiplier::Double)
            );
        }
    }

    #[test]
    fn zero_or_negative_overtime_hours_are_refused() {
        assert_eq!(
            OvertimeHours::new(dec!(0)),
            Err(OvertimeHoursError::ZeroOrNegative)
        );
        assert_eq!(
            OvertimeHours::new(dec!(-0.25)),
            Err(OvertimeHoursError::ZeroOrNegative)
        );
    }

    #[test]
    fn overtime_hours_are_bounded_and_cents_exact() {
        assert_eq!(
            OvertimeHours::new(MAX_OVERTIME_HOURS + dec!(0.01)),
            Err(OvertimeHoursError::MoreThanAWholePeriod {
                maximum: MAX_OVERTIME_HOURS
            })
        );
        assert_eq!(
            OvertimeHours::new(dec!(1.005)),
            Err(OvertimeHoursError::MoreThanTwoDecimalPlaces)
        );
        assert!(OvertimeHours::new(MAX_OVERTIME_HOURS).is_ok());
    }

    #[test]
    fn an_overtime_instruction_and_line_round_trip() {
        let instruction = EarningInstruction::Overtime {
            hours: OvertimeHours::new(dec!(12)).unwrap(),
            multiplier: OvertimeMultiplier::OneAndAHalf,
            label: Some(EarningLabel::new("Sunday overtime").unwrap()),
        };
        let json = serde_json::to_string(&instruction).unwrap();
        assert_eq!(
            serde_json::from_str::<EarningInstruction>(&json).unwrap(),
            instruction
        );

        let line = Earning::Overtime {
            amount: money(dec!(1246.15)),
            trace: a_trace(),
            label: Some(EarningLabel::new("Sunday overtime").unwrap()),
        };
        let json = serde_json::to_string(&line).unwrap();
        assert_eq!(serde_json::from_str::<Earning>(&json).unwrap(), line);
    }

    /// An instruction carries hours; a line carries money. Reading one
    /// shape as the other must fail rather than silently lose the money or
    /// invent it.
    #[test]
    fn an_overtime_line_is_not_readable_as_an_overtime_instruction() {
        let line = serde_json::to_value(Earning::Overtime {
            amount: money(dec!(1246.15)),
            trace: a_trace(),
            label: None,
        })
        .unwrap();

        assert!(serde_json::from_value::<EarningInstruction>(line).is_err());
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
