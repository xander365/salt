//! The Employment facts the calculator needs for one PayPeriod.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

/// The weekly ordinary hours a monthly `BasicPay` covers.
///
/// This is deliberately a distinct exact-decimal value rather than a bare
/// `Decimal`: it is an agreed contractual assumption for a future hourly
/// rate, never hours worked or paid in a payroll period.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Decimal", into = "Decimal")]
pub struct OrdinaryHours(Decimal);

/// Why an [`OrdinaryHours`] value cannot be recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrdinaryHoursError {
    ZeroOrNegative,
    MoreThanOneWeek,
    MoreThanTwoDecimalPlaces,
}

impl std::fmt::Display for OrdinaryHoursError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroOrNegative => write!(f, "ordinary hours must be greater than zero"),
            Self::MoreThanOneWeek => write!(f, "ordinary hours must not exceed 168 per week"),
            Self::MoreThanTwoDecimalPlaces => {
                write!(
                    f,
                    "ordinary hours must have no more than two decimal places"
                )
            }
        }
    }
}

impl std::error::Error for OrdinaryHoursError {}

impl OrdinaryHours {
    pub fn new(hours: Decimal) -> Result<Self, OrdinaryHoursError> {
        if hours <= Decimal::ZERO {
            return Err(OrdinaryHoursError::ZeroOrNegative);
        }
        if hours > Decimal::new(168, 0) {
            return Err(OrdinaryHoursError::MoreThanOneWeek);
        }
        if hours.scale() > 2 {
            return Err(OrdinaryHoursError::MoreThanTwoDecimalPlaces);
        }
        Ok(Self(hours))
    }

    pub fn as_decimal(self) -> Decimal {
        self.0
    }
}

impl TryFrom<Decimal> for OrdinaryHours {
    type Error = OrdinaryHoursError;

    fn try_from(hours: Decimal) -> Result<Self, Self::Error> {
        Self::new(hours)
    }
}

impl From<OrdinaryHours> for Decimal {
    fn from(hours: OrdinaryHours) -> Self {
        hours.0
    }
}

use crate::money::Money;
use crate::pay_period::PayPeriod;

/// Defines an opaque string-backed identifier. The identifiers in this
/// module differ only in which entity they name, and that difference is
/// the whole point of having three types rather than one `String`.
macro_rules! identifier {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name(String);

        impl $name {
            pub fn new(id: impl Into<String>) -> Self {
                $name(id.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

identifier! {
    /// Identifies which Employment a `PayrollCalculation` was produced for.
    EmploymentId
}

identifier! {
    /// Identifies the Employer the Employment is with. Carried from day one
    /// so more than one Employer per workspace is additive later.
    EmployerId
}

identifier! {
    /// Identifies the Person the Employment is for.
    PersonId
}

/// Names the Person an Employment is for without copying any personal
/// information into the payroll domain: the calculator never needs a name,
/// an identity number, or contact details, so a frozen `PayrollInput` must
/// not carry them.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PersonReference(PersonId);

impl PersonReference {
    pub fn new(person_id: PersonId) -> Self {
        PersonReference(person_id)
    }

    pub fn person_id(&self) -> &PersonId {
        &self.0
    }
}

/// Why a `CompensationTerms` value could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompensationTermsError {
    /// `effective_until` was before `effective_from`, so the terms are in
    /// force for no day at all.
    EndBeforeStart,
}

impl std::fmt::Display for CompensationTermsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CompensationTermsError::EndBeforeStart => {
                write!(f, "effective_until is before effective_from")
            }
        }
    }
}

impl std::error::Error for CompensationTermsError {}

/// What the Employment agrees to pay, over an effective period. v1
/// supports monthly `BasicPay` only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawCompensationTerms", into = "RawCompensationTerms")]
pub struct CompensationTerms {
    effective_from: NaiveDate,
    effective_until: Option<NaiveDate>,
    basic_pay: Money,
    ordinary_hours: Option<OrdinaryHours>,
}

/// The wire shape of `CompensationTerms`, validated on the way in by the
/// `TryFrom` impl so deserialization cannot bypass the effective-period
/// invariant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawCompensationTerms {
    pub effective_from: NaiveDate,
    pub effective_until: Option<NaiveDate>,
    pub basic_pay: Money,
    #[serde(default)]
    pub ordinary_hours: Option<OrdinaryHours>,
}

impl CompensationTerms {
    pub fn new(
        effective_from: NaiveDate,
        effective_until: Option<NaiveDate>,
        basic_pay: Money,
    ) -> Result<Self, CompensationTermsError> {
        if effective_until.is_some_and(|until| until < effective_from) {
            return Err(CompensationTermsError::EndBeforeStart);
        }
        Ok(CompensationTerms {
            effective_from,
            effective_until,
            basic_pay,
            ordinary_hours: None,
        })
    }

    pub fn effective_from(&self) -> NaiveDate {
        self.effective_from
    }

    pub fn effective_until(&self) -> Option<NaiveDate> {
        self.effective_until
    }

    pub fn basic_pay(&self) -> Money {
        self.basic_pay
    }

    pub fn ordinary_hours(&self) -> Option<OrdinaryHours> {
        self.ordinary_hours
    }

    /// Attaches the agreed weekly hours read from the effective-dated row.
    /// `None` remains meaningful for historical rows Salt must not invent.
    pub fn with_ordinary_hours(mut self, ordinary_hours: Option<OrdinaryHours>) -> Self {
        self.ordinary_hours = ordinary_hours;
        self
    }

    /// Whether these terms are in force for every day from `from` to `to`
    /// inclusive — the days actually being paid for, not the whole
    /// `PayPeriod`. For a continuing employee the two are the same span.
    /// For a leaver they are not: terms that end on the employee's last
    /// day cover every day that is paid, and refusing that ordinary case
    /// would make a correctly recorded leaver uncalculable.
    pub(crate) fn cover_days(&self, from: NaiveDate, to: NaiveDate) -> bool {
        self.effective_from <= from && self.effective_until.is_none_or(|until| until >= to)
    }
}

impl TryFrom<RawCompensationTerms> for CompensationTerms {
    type Error = CompensationTermsError;

    fn try_from(raw: RawCompensationTerms) -> Result<Self, CompensationTermsError> {
        CompensationTerms::new(raw.effective_from, raw.effective_until, raw.basic_pay)
            .map(|terms| terms.with_ordinary_hours(raw.ordinary_hours))
    }
}

impl From<CompensationTerms> for RawCompensationTerms {
    fn from(terms: CompensationTerms) -> RawCompensationTerms {
        RawCompensationTerms {
            effective_from: terms.effective_from,
            effective_until: terms.effective_until,
            basic_pay: terms.basic_pay,
            ordinary_hours: terms.ordinary_hours,
        }
    }
}

/// The days of one `PayPeriod` an Employment was actually active for.
/// Always non-empty: `first <= last`, because the only way to obtain one
/// is `EmploymentSnapshot::employed_days_within`, which returns `None`
/// rather than an empty span.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EmployedSpan {
    first: NaiveDate,
    last: NaiveDate,
}

impl EmployedSpan {
    pub(crate) fn first(self) -> NaiveDate {
        self.first
    }

    pub(crate) fn last(self) -> NaiveDate {
        self.last
    }

    /// The inclusive day count. At least 1, and bounded by the length of
    /// the `PayPeriod` it was taken from.
    pub(crate) fn days(self) -> i64 {
        (self.last - self.first).num_days() + 1
    }
}

/// The Employment facts relevant to calculating one PayPeriod, captured so
/// a historical result is never reinterpreted through today's employment
/// record. The Employer and Person are named by reference only — the
/// calculator never reads them, but a frozen `PayrollInput` must record
/// whose Employment was calculated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmploymentSnapshot {
    employment_id: EmploymentId,
    employer_id: EmployerId,
    person: PersonReference,
    start_date: NaiveDate,
    end_date: Option<NaiveDate>,
    compensation_terms: CompensationTerms,
}

impl EmploymentSnapshot {
    pub fn new(
        employment_id: EmploymentId,
        employer_id: EmployerId,
        person: PersonReference,
        start_date: NaiveDate,
        end_date: Option<NaiveDate>,
        compensation_terms: CompensationTerms,
    ) -> Self {
        EmploymentSnapshot {
            employment_id,
            employer_id,
            person,
            start_date,
            end_date,
            compensation_terms,
        }
    }

    pub fn employment_id(&self) -> &EmploymentId {
        &self.employment_id
    }

    pub fn employer_id(&self) -> &EmployerId {
        &self.employer_id
    }

    pub fn person(&self) -> &PersonReference {
        &self.person
    }

    pub fn start_date(&self) -> NaiveDate {
        self.start_date
    }

    pub fn end_date(&self) -> Option<NaiveDate> {
        self.end_date
    }

    pub fn compensation_terms(&self) -> CompensationTerms {
        self.compensation_terms
    }

    /// Whether `start_date` and `end_date` are internally coherent.
    pub(crate) fn has_coherent_dates(&self) -> bool {
        self.end_date.is_none_or(|end| end >= self.start_date)
    }

    /// The first and last day, inclusive, on which this Employment was
    /// active within `period` — the days actually being paid for, and the
    /// numerator `calculate` uses to prorate `BasicPay` for a joiner or a
    /// leaver. `None` if the Employment does not overlap `period` at all,
    /// which is a genuine mismatch rather than an ordinary joiner or
    /// leaver: those are not errors (`docs/domain/payroll-calculation.md`
    /// §8.2), but an Employment wholly before or after the period being
    /// calculated is.
    ///
    /// Callers get the dates, not just a count, because the same span
    /// answers a second question — which days the `CompensationTerms`
    /// must be in force for (see `CompensationTerms::cover_days`).
    pub(crate) fn employed_days_within(&self, period: PayPeriod) -> Option<EmployedSpan> {
        let first = self.start_date.max(period.start());
        let last = self
            .end_date
            .map_or(period.end(), |end| end.min(period.end()));
        if first > last {
            None
        } else {
            Some(EmployedSpan { first, last })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    fn terms() -> CompensationTerms {
        CompensationTerms::new(date(2025, 1, 1), None, Money::from_cents(500000).unwrap()).unwrap()
    }

    #[test]
    fn ordinary_hours_are_exact_positive_weekly_hours() {
        use rust_decimal_macros::dec;

        assert_eq!(
            OrdinaryHours::new(dec!(40.005)),
            Err(OrdinaryHoursError::MoreThanTwoDecimalPlaces)
        );
        assert_eq!(
            OrdinaryHours::new(dec!(0)),
            Err(OrdinaryHoursError::ZeroOrNegative)
        );
        assert_eq!(
            OrdinaryHours::new(dec!(168.01)),
            Err(OrdinaryHoursError::MoreThanOneWeek)
        );
        assert_eq!(
            OrdinaryHours::new(dec!(40.00)).unwrap().as_decimal(),
            dec!(40.00)
        );
    }

    #[test]
    fn compensation_terms_round_trip_keeps_ordinary_hours() {
        use rust_decimal_macros::dec;

        let terms =
            CompensationTerms::new(date(2025, 1, 1), None, Money::from_cents(500000).unwrap())
                .unwrap()
                .with_ordinary_hours(Some(OrdinaryHours::new(dec!(40.00)).unwrap()));

        let reloaded: CompensationTerms = serde_json::from_str(
            &serde_json::to_string(&terms).expect("CompensationTerms serializes"),
        )
        .expect("CompensationTerms deserializes");

        assert_eq!(reloaded.ordinary_hours(), terms.ordinary_hours());
    }

    fn snapshot() -> EmploymentSnapshot {
        EmploymentSnapshot::new(
            EmploymentId::new("emp-1"),
            EmployerId::new("employer-1"),
            PersonReference::new(PersonId::new("person-1")),
            date(2025, 1, 1),
            None,
            terms(),
        )
    }

    #[test]
    fn compensation_terms_accept_an_open_ended_effective_period() {
        assert!(CompensationTerms::new(date(2025, 1, 1), None, Money::ZERO).is_ok());
    }

    #[test]
    fn compensation_terms_accept_a_single_day_effective_period() {
        assert!(
            CompensationTerms::new(date(2025, 1, 1), Some(date(2025, 1, 1)), Money::ZERO).is_ok()
        );
    }

    #[test]
    fn compensation_terms_reject_an_end_before_their_start() {
        assert_eq!(
            CompensationTerms::new(date(2025, 3, 1), Some(date(2025, 1, 1)), Money::ZERO),
            Err(CompensationTermsError::EndBeforeStart)
        );
    }

    #[test]
    fn compensation_terms_deserialize_rejects_an_end_before_its_start() {
        let json = serde_json::to_string(&RawCompensationTerms {
            effective_from: date(2025, 3, 1),
            effective_until: Some(date(2025, 1, 1)),
            basic_pay: Money::ZERO,
            ordinary_hours: None,
        })
        .unwrap();
        assert!(serde_json::from_str::<CompensationTerms>(&json).is_err());
    }

    #[test]
    fn the_snapshot_names_the_employer_and_the_person() {
        let snapshot = snapshot();
        assert_eq!(snapshot.employer_id().as_str(), "employer-1");
        assert_eq!(snapshot.person().person_id().as_str(), "person-1");
    }

    // `employed_days_within` and `cover_days` are exercised through the
    // `calculate` seam — see PC-005, PC-006, the leaver whose terms end on
    // their last day, and the no-overlap refusals in `calculation.rs`.

    #[test]
    fn deserialize_round_trips() {
        let snapshot = snapshot();
        let json = serde_json::to_string(&snapshot).unwrap();
        assert_eq!(
            serde_json::from_str::<EmploymentSnapshot>(&json).unwrap(),
            snapshot
        );
    }
}
