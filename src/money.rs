//! Exact-decimal money at cents precision (INV-001: never `f32`/`f64`).

use rust_decimal::{Decimal, RoundingStrategy};

/// An amount of money, exact to the cent, never negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Money(i64);

/// Why a `Money` value could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoneyError {
    /// The amount was negative.
    Negative,
    /// The decimal carried more than two decimal places, so it is not an
    /// exact number of cents.
    FractionalCents,
}

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
        let cents = amount.mantissa() * 10i128.pow(2 - amount.scale());
        let cents: i64 = cents.try_into().expect("payroll amounts fit in i64 cents");
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
}

/// Rounds an exact decimal amount half-up to two decimal places and
/// constructs the resulting `Money`.
///
/// This is the one rounding mechanism the crate provides; when and whether
/// to apply it is a policy decision that belongs to `PayrollRules`, not here.
pub fn round_half_up(amount: Decimal) -> Result<Money, MoneyError> {
    let cents = (amount * Decimal::ONE_HUNDRED)
        .round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero);
    let cents: i64 = cents.try_into().expect("payroll amounts fit in i64 cents");
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
}
