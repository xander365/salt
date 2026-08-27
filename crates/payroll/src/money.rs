//! Exact-decimal money at cents precision (INV-001: never `f32`/`f64`).

use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};

/// An amount of money, exact to the cent, never negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct Money(i64);

/// Why a `Money` value could not be constructed or produced by an
/// arithmetic operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoneyError {
    /// The amount was negative.
    Negative,
    /// The decimal carried more than two decimal places, so it is not an
    /// exact number of cents.
    FractionalCents,
    /// The amount does not fit in an `i64` number of cents.
    Overflow,
}

impl std::fmt::Display for MoneyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MoneyError::Negative => write!(f, "amount is negative"),
            MoneyError::FractionalCents => write!(f, "amount is not exact at cents precision"),
            MoneyError::Overflow => write!(f, "amount does not fit in i64 cents"),
        }
    }
}

impl std::error::Error for MoneyError {}

impl Money {
    pub const ZERO: Money = Money(0);

    /// Constructs a `Money` value directly from a whole number of cents.
    pub fn from_cents(cents: i64) -> Result<Self, MoneyError> {
        if cents < 0 {
            Err(MoneyError::Negative)
        } else {
            Ok(Money(cents))
        }
    }

    /// Constructs a `Money` value from a decimal amount that is already
    /// exact at cents precision. Use [`round_half_up`] instead when the
    /// amount carries more precision and needs rounding.
    pub fn from_decimal(amount: Decimal) -> Result<Self, MoneyError> {
        if amount.scale() > 2 {
            return Err(MoneyError::FractionalCents);
        }
        let cents = amount
            .mantissa()
            .checked_mul(10i128.pow(2 - amount.scale()))
            .ok_or(MoneyError::Overflow)?;
        let cents: i64 = cents.try_into().map_err(|_| MoneyError::Overflow)?;
        Money::from_cents(cents)
    }

    /// The amount as a whole number of cents.
    pub fn cents(self) -> i64 {
        self.0
    }

    /// The amount as an exact decimal, e.g. `Decimal::new(1050, 2)` for N$10.50.
    pub fn as_decimal(self) -> Decimal {
        Decimal::new(self.0, 2)
    }

    /// Adds `other`, or `Err(MoneyError::Overflow)` if the sum does not fit
    /// in `i64` cents.
    pub fn checked_add(self, other: Money) -> Result<Money, MoneyError> {
        self.0
            .checked_add(other.0)
            .map(Money)
            .ok_or(MoneyError::Overflow)
    }

    /// Subtracts `other`, or `Err(MoneyError::Negative)` if that would go
    /// below zero. Money can never represent a negative amount, so
    /// subtraction is the operation most likely to fail.
    pub fn checked_sub(self, other: Money) -> Result<Money, MoneyError> {
        if self.0 < other.0 {
            Err(MoneyError::Negative)
        } else {
            Ok(Money(self.0 - other.0))
        }
    }

    /// Sums an iterator of amounts, or `Err(MoneyError::Overflow)` as soon
    /// as a running total would not fit in `i64` cents.
    pub fn checked_sum(amounts: impl IntoIterator<Item = Money>) -> Result<Money, MoneyError> {
        amounts
            .into_iter()
            .try_fold(Money::ZERO, |total, amount| total.checked_add(amount))
    }
}

impl TryFrom<i64> for Money {
    type Error = MoneyError;

    fn try_from(cents: i64) -> Result<Self, MoneyError> {
        Money::from_cents(cents)
    }
}

impl From<Money> for i64 {
    fn from(money: Money) -> i64 {
        money.0
    }
}

/// Rounds an exact decimal amount half-up to two decimal places and
/// constructs the resulting `Money`.
///
/// This is the one rounding mechanism the crate provides; when and whether
/// to apply it is a policy decision that belongs to `PayrollRules`, not here.
pub fn round_half_up(amount: Decimal) -> Result<Money, MoneyError> {
    let scaled = amount
        .checked_mul(Decimal::ONE_HUNDRED)
        .ok_or(MoneyError::Overflow)?
        .round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero);
    let cents: i64 = scaled.try_into().map_err(|_| MoneyError::Overflow)?;
    Money::from_cents(cents)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn zero_is_zero_cents() {
        assert_eq!(Money::ZERO.cents(), 0);
    }

    #[test]
    fn from_cents_rejects_negative() {
        assert_eq!(Money::from_cents(-1), Err(MoneyError::Negative));
    }

    #[test]
    fn from_cents_accepts_zero_and_positive() {
        assert_eq!(Money::from_cents(0).unwrap().cents(), 0);
        assert_eq!(Money::from_cents(150).unwrap().cents(), 150);
    }

    #[test]
    fn from_decimal_accepts_exact_cents() {
        let money = Money::from_decimal(dec!(10.50)).unwrap();
        assert_eq!(money.cents(), 1050);
        assert_eq!(money.as_decimal(), dec!(10.50));
    }

    #[test]
    fn from_decimal_accepts_whole_amounts() {
        let money = Money::from_decimal(dec!(500)).unwrap();
        assert_eq!(money.cents(), 50000);
    }

    #[test]
    fn from_decimal_rejects_fractional_cents() {
        assert_eq!(
            Money::from_decimal(dec!(10.505)),
            Err(MoneyError::FractionalCents)
        );
    }

    #[test]
    fn from_decimal_rejects_negative() {
        assert_eq!(Money::from_decimal(dec!(-1.00)), Err(MoneyError::Negative));
    }

    #[test]
    fn round_half_up_rounds_up_at_the_boundary() {
        // 1.005 sits exactly halfway between 1.00 and 1.01.
        let money = round_half_up(dec!(1.005)).unwrap();
        assert_eq!(money.as_decimal(), dec!(1.01));
    }

    #[test]
    fn round_half_up_rounds_up_across_a_carry_boundary() {
        // The other direction: rounding up here also carries into the next
        // whole unit, not just the next cent.
        let money = round_half_up(dec!(1.995)).unwrap();
        assert_eq!(money.as_decimal(), dec!(2.00));
    }

    #[test]
    fn round_half_up_does_not_round_down_a_true_half() {
        // Exact decimal arithmetic: unlike f64, 2.675 is exactly 2.675 here,
        // not 2.67499999...
        let money = round_half_up(dec!(2.675)).unwrap();
        assert_eq!(money.as_decimal(), dec!(2.68));
    }

    #[test]
    fn round_half_up_leaves_non_boundary_values_alone() {
        assert_eq!(round_half_up(dec!(1.004)).unwrap().as_decimal(), dec!(1.00));
        assert_eq!(round_half_up(dec!(1.006)).unwrap().as_decimal(), dec!(1.01));
    }

    #[test]
    fn round_half_up_rejects_negative_results() {
        assert_eq!(round_half_up(dec!(-0.01)), Err(MoneyError::Negative));
    }

    #[test]
    fn checked_add_is_exact() {
        let a = Money::from_cents(150).unwrap();
        let b = Money::from_cents(250).unwrap();
        assert_eq!(a.checked_add(b).unwrap().cents(), 400);
    }

    #[test]
    fn checked_add_rejects_overflow_at_i64_max() {
        let a = Money::from_cents(i64::MAX).unwrap();
        let b = Money::from_cents(1).unwrap();
        assert_eq!(a.checked_add(b), Err(MoneyError::Overflow));
    }

    #[test]
    fn checked_add_accepts_up_to_i64_max() {
        let a = Money::from_cents(i64::MAX - 1).unwrap();
        let b = Money::from_cents(1).unwrap();
        assert_eq!(a.checked_add(b).unwrap().cents(), i64::MAX);
    }

    #[test]
    fn checked_sub_is_exact_when_it_does_not_go_below_zero() {
        let a = Money::from_cents(400).unwrap();
        let b = Money::from_cents(150).unwrap();
        assert_eq!(a.checked_sub(b).unwrap().cents(), 250);
    }

    #[test]
    fn checked_sub_rejects_going_below_zero() {
        let a = Money::from_cents(100).unwrap();
        let b = Money::from_cents(101).unwrap();
        assert_eq!(a.checked_sub(b), Err(MoneyError::Negative));
    }

    #[test]
    fn checked_sum_folds_from_zero() {
        let amounts = vec![
            Money::from_cents(100).unwrap(),
            Money::from_cents(200).unwrap(),
            Money::from_cents(300).unwrap(),
        ];
        assert_eq!(Money::checked_sum(amounts).unwrap().cents(), 600);
    }

    #[test]
    fn checked_sum_of_empty_is_zero() {
        assert_eq!(Money::checked_sum(Vec::new()).unwrap(), Money::ZERO);
    }

    #[test]
    fn checked_sum_rejects_overflow() {
        let amounts = vec![
            Money::from_cents(i64::MAX).unwrap(),
            Money::from_cents(1).unwrap(),
        ];
        assert_eq!(Money::checked_sum(amounts), Err(MoneyError::Overflow));
    }

    #[test]
    fn serializes_as_a_plain_integer() {
        let money = Money::from_cents(1050).unwrap();
        assert_eq!(serde_json::to_string(&money).unwrap(), "1050");
    }

    #[test]
    fn deserialize_round_trips() {
        let money = Money::from_cents(1050).unwrap();
        let json = serde_json::to_string(&money).unwrap();
        assert_eq!(serde_json::from_str::<Money>(&json).unwrap(), money);
    }

    #[test]
    fn deserialize_rejects_a_negative_amount() {
        assert!(serde_json::from_str::<Money>("-1").is_err());
    }
}
