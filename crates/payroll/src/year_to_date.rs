//! `YearToDateContext`: what has happened so far this TaxYear for one
//! Employment, supplied to the calculator rather than queried by it.

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};

use crate::money::Money;
use crate::tax_year::TaxYear;

/// The Employment's **position in the TaxYear**, 0 to 11 — how many of
/// the TaxYear's twelve periods are already behind this one.
///
/// **Not a count of periods the Employment has been paid, and not a count
/// of days worked.** An Employment that starts in October is at position
/// 7, not at position 0, even though Salt has never paid it before. This
/// is Salt policy under an unprescribed per-period method (SC-OPEN-1,
/// ADR-0001, `docs/domain/statutory-conformance.md` §5.2), and it is the
/// single reading in this crate most likely to be "corrected" into a bug.
///
/// **The failure a misreading causes is over-withholding, every time.**
/// The annual band thresholds are scaled to `period_number`/12. Reading
/// this as periods *worked* would put an October joiner at position 0,
/// scale October's thresholds to 1/12 instead of 8/12, and tax them
/// immediately against a fraction of the tax-free threshold they are
/// actually entitled to — withholding more than their true annual
/// liability. Tax-year position does not under-tax them in exchange: by
/// February the thresholds are whole and the correct annual amount has
/// been collected. The method self-corrects; counting worked periods
/// breaks it. Do not change this.
///
/// The genuine under-taxation risk is a different fact entirely, and
/// [`PriorEmployment`] closes it by refusing.
///
/// Keying the ruleset and the TaxYear on the PayPeriod end date guarantees
/// exactly twelve periods per TaxYear (ADR-0005), and cumulative PAYE
/// depends on that. A thirteenth period would scale the thresholds past
/// the full annual table and under-tax the employee, so 12 and above are
/// not representable rather than merely wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct PeriodsElapsed(u8);

/// Why a `PeriodsElapsed` could not be constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPeriodsElapsed(pub u8);

impl std::fmt::Display for InvalidPeriodsElapsed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} is not a count of elapsed periods in 0..=11 — a TaxYear has exactly twelve periods",
            self.0
        )
    }
}

impl std::error::Error for InvalidPeriodsElapsed {}

impl PeriodsElapsed {
    /// No period of this TaxYear is complete yet.
    pub const NONE: PeriodsElapsed = PeriodsElapsed(0);

    pub fn new(elapsed: u8) -> Result<Self, InvalidPeriodsElapsed> {
        if elapsed <= 11 {
            Ok(PeriodsElapsed(elapsed))
        } else {
            Err(InvalidPeriodsElapsed(elapsed))
        }
    }

    pub fn get(self) -> u8 {
        self.0
    }

    /// The position a `PayPeriod` occupies in its own `TaxYear`, derived
    /// from the period's end date alone — see the warning on this type for
    /// what that position is and is not.
    ///
    /// `TaxYear::for_period_end` places every period end date in exactly
    /// one of the tax year's twelve calendar months, so the position is
    /// always representable and this never fails.
    pub fn from_period_end(period_end: NaiveDate) -> Self {
        let tax_year = TaxYear::for_period_end(period_end);
        let months_since_march =
            (period_end.year() - tax_year.starting_year()) * 12 + period_end.month() as i32 - 3;
        let elapsed = u8::try_from(months_since_march)
            .expect("a PayPeriod end date's position in its own TaxYear is always 0..=11");
        PeriodsElapsed::new(elapsed)
            .expect("a PayPeriod end date's position in its own TaxYear is always 0..=11")
    }

    /// Which period of the TaxYear is being calculated, counting the
    /// current one: 1 for the first, 12 for the last. This is the figure
    /// the PAYE band thresholds are scaled by (`period_number`/12) — what
    /// ADR-0001 calls the periods elapsed *inclusive of this one*.
    pub(crate) fn period_number(self) -> u32 {
        self.0 as u32 + 1
    }
}

impl TryFrom<u8> for PeriodsElapsed {
    type Error = InvalidPeriodsElapsed;

    fn try_from(elapsed: u8) -> Result<Self, InvalidPeriodsElapsed> {
        PeriodsElapsed::new(elapsed)
    }
}

impl From<PeriodsElapsed> for u8 {
    fn from(elapsed: PeriodsElapsed) -> u8 {
        elapsed.0
    }
}

/// The taxable remuneration and PAYE from a Person's tax certificate
/// for taxable employment with another Employer earlier in the same tax
/// year. Carried by `PriorEmployment::Some` and into
/// `PayrollError::PriorEmploymentPresent`, so nothing has to be
/// re-gathered once SC-OPEN-4 is resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PriorEmploymentFigures {
    taxable_remuneration: Money,
    paye: Money,
}

impl PriorEmploymentFigures {
    pub fn new(taxable_remuneration: Money, paye: Money) -> Self {
        PriorEmploymentFigures {
            taxable_remuneration,
            paye,
        }
    }

    pub fn taxable_remuneration(self) -> Money {
        self.taxable_remuneration
    }

    pub fn paye(self) -> Money {
        self.paye
    }
}

/// Whether the Person had taxable employment with a *different*
/// Employer earlier in the same tax year.
///
/// This is never the same fact as an `OpeningBalance`
/// (`prior_taxable_remuneration`, `prior_paye`, `periods_elapsed` above):
/// an `OpeningBalance` is prior year-to-date figures for *this same*
/// Employment and Employer, from before Salt — a mid-year system
/// replacement, settled and unaffected by this type (ADR-0001).
/// `PriorEmployment` is taxable employment with a *different* Employer,
/// and the two must never be conflated.
///
/// Explicitly three-valued so a zero can never be mistaken for "nobody
/// asked" (SC-OPEN-4). There is no Salt policy for `Some` — only a
/// refusal: how a new employer must treat another employer's remuneration
/// and PAYE, and whether a NamRA directive or certificate is required
/// first, is unresolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PriorEmployment {
    /// Confirmed: no earlier taxable employment this tax year.
    /// `calculate` proceeds under Salt's documented policy.
    None,
    /// Figures from the Person's tax certificate. `calculate` refuses,
    /// carrying the figures into the typed error.
    Some(PriorEmploymentFigures),
    /// Nobody has established the fact. `calculate` refuses rather than
    /// treat an unasked question as a confirmed `None`.
    Unknown,
}

/// The TaxYear, taxable remuneration, PAYE withheld, periods elapsed, and
/// PriorEmployment fact so far for one Employment.
///
/// Required, never optional (ADR-0001): the first period of adoption
/// passes explicit zeros, not a missing value. There is therefore no
/// "missing year-to-date context" refusal for `calculate` to return — the
/// type does not allow the state. The figures are summed by the
/// application layer from live, non-reversed `FinalizedPayroll` records
/// plus the Employment's `OpeningBalance`, never stored as a running
/// total (INV-013), and `calculate` never queries them.
///
/// **Two different facts live here, and they are never the same fact.**
/// `prior_taxable_remuneration`, `prior_paye` and `periods_elapsed` are
/// the **`OpeningBalance`** axis — this same Employment and Employer,
/// carried in from whatever system Salt replaced. That is **supported**.
/// `prior_employment` is the [`PriorEmployment`] axis — a *different*
/// Employer earlier in the same TaxYear. That is **refused** while
/// SC-OPEN-4 is open. Conflating them is how a mid-year joiner gets
/// materially under-taxed
/// (`docs/domain/statutory-conformance.md` §5.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct YearToDateContext {
    tax_year: TaxYear,
    /// `OpeningBalance` axis: taxable remuneration already paid by **this
    /// same** Employment and Employer this TaxYear. Never another
    /// employer's figures — those are `prior_employment`, and they are
    /// refused.
    prior_taxable_remuneration: Money,
    /// `OpeningBalance` axis: PAYE already withheld by **this same**
    /// Employment and Employer this TaxYear.
    prior_paye: Money,
    /// Position in the TaxYear — see [`PeriodsElapsed`]. An `OpeningBalance`
    /// supplies it on adoption; it is not a count of periods Salt has paid.
    periods_elapsed: PeriodsElapsed,
    /// The separate, refused fact — see [`PriorEmployment`].
    prior_employment: PriorEmployment,
}

impl YearToDateContext {
    pub fn new(
        tax_year: TaxYear,
        prior_taxable_remuneration: Money,
        prior_paye: Money,
        periods_elapsed: PeriodsElapsed,
        prior_employment: PriorEmployment,
    ) -> Self {
        YearToDateContext {
            tax_year,
            prior_taxable_remuneration,
            prior_paye,
            periods_elapsed,
            prior_employment,
        }
    }

    /// The first period of a tax year after the caller has established
    /// that the Person had no prior employment in that tax year. The
    /// explicit name prevents an unanswered prior-employment question from
    /// silently becoming a confirmed `PriorEmployment::None`.
    pub fn first_period_with_no_prior_employment(tax_year: TaxYear) -> Self {
        YearToDateContext::new(
            tax_year,
            Money::ZERO,
            Money::ZERO,
            PeriodsElapsed::NONE,
            PriorEmployment::None,
        )
    }

    pub fn tax_year(self) -> TaxYear {
        self.tax_year
    }

    pub fn prior_taxable_remuneration(self) -> Money {
        self.prior_taxable_remuneration
    }

    pub fn prior_paye(self) -> Money {
        self.prior_paye
    }

    pub fn periods_elapsed(self) -> PeriodsElapsed {
        self.periods_elapsed
    }

    pub fn prior_employment(self) -> PriorEmployment {
        self.prior_employment
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods_elapsed_accepts_0_to_11() {
        assert_eq!(PeriodsElapsed::new(0).unwrap().get(), 0);
        assert_eq!(PeriodsElapsed::new(11).unwrap().get(), 11);
    }

    #[test]
    fn periods_elapsed_rejects_a_thirteenth_period() {
        assert_eq!(PeriodsElapsed::new(12), Err(InvalidPeriodsElapsed(12)));
        assert_eq!(PeriodsElapsed::new(255), Err(InvalidPeriodsElapsed(255)));
    }

    #[test]
    fn period_number_counts_the_current_period() {
        assert_eq!(PeriodsElapsed::NONE.period_number(), 1);
        assert_eq!(PeriodsElapsed::new(11).unwrap().period_number(), 12);
    }

    #[test]
    fn periods_elapsed_deserialize_rejects_out_of_range() {
        assert!(serde_json::from_str::<PeriodsElapsed>("12").is_err());
    }

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    #[test]
    fn the_first_period_of_a_tax_year_has_position_zero() {
        assert_eq!(
            PeriodsElapsed::from_period_end(date(2026, 3, 25)),
            PeriodsElapsed::new(0).unwrap()
        );
    }

    #[test]
    fn a_middle_period_of_a_tax_year_has_its_month_offset_from_march_as_position() {
        // September: six full months after the tax year's March start.
        assert_eq!(
            PeriodsElapsed::from_period_end(date(2026, 9, 25)),
            PeriodsElapsed::new(6).unwrap()
        );
    }

    #[test]
    fn the_last_period_of_a_tax_year_has_position_eleven() {
        // February belongs to the tax year that started the previous
        // March (ADR-0005), and is that tax year's twelfth period.
        assert_eq!(
            PeriodsElapsed::from_period_end(date(2027, 2, 25)),
            PeriodsElapsed::new(11).unwrap()
        );
    }

    // The whole point of this constructor is that it reads position, never
    // a count of what has been paid. A September end date yields position 6
    // even for an Employment that has never been paid before — an October
    // joiner's very first payroll run is not position 0.
    #[test]
    fn from_period_end_is_a_position_not_a_count_of_periods_paid() {
        let first_ever_run_for_a_september_joiner = date(2026, 9, 25);
        assert_eq!(
            PeriodsElapsed::from_period_end(first_ever_run_for_a_september_joiner),
            PeriodsElapsed::new(6).unwrap()
        );
    }

    #[test]
    fn first_period_with_no_prior_employment_is_all_zeros() {
        let ytd = YearToDateContext::first_period_with_no_prior_employment(TaxYear::starting(2026));
        assert_eq!(ytd.prior_taxable_remuneration(), Money::ZERO);
        assert_eq!(ytd.prior_paye(), Money::ZERO);
        assert_eq!(ytd.periods_elapsed(), PeriodsElapsed::NONE);
        assert_eq!(ytd.prior_employment(), PriorEmployment::None);
    }

    #[test]
    fn prior_employment_figures_expose_the_recorded_amounts() {
        let figures = PriorEmploymentFigures::new(
            Money::from_cents(150_000).unwrap(),
            Money::from_cents(20_000).unwrap(),
        );
        assert_eq!(
            figures.taxable_remuneration(),
            Money::from_cents(150_000).unwrap()
        );
        assert_eq!(figures.paye(), Money::from_cents(20_000).unwrap());
    }

    #[test]
    fn prior_employment_round_trips_through_all_three_states() {
        let states = [
            PriorEmployment::None,
            PriorEmployment::Unknown,
            PriorEmployment::Some(PriorEmploymentFigures::new(
                Money::from_cents(150_000).unwrap(),
                Money::from_cents(20_000).unwrap(),
            )),
        ];
        for state in states {
            let json = serde_json::to_string(&state).unwrap();
            assert_eq!(
                serde_json::from_str::<PriorEmployment>(&json).unwrap(),
                state
            );
        }
    }

    // The whole point of the fact is that an unasked question cannot pass
    // as a confirmed `None`. A wire format that let the field be omitted
    // would put that ambiguity straight back, so absence is a hard error.
    #[test]
    fn deserialize_requires_the_prior_employment_fact() {
        let ytd = YearToDateContext::new(
            TaxYear::starting(2026),
            Money::from_cents(100).unwrap(),
            Money::from_cents(10).unwrap(),
            PeriodsElapsed::new(3).unwrap(),
            PriorEmployment::None,
        );
        let mut fields: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&ytd).unwrap()).unwrap();
        // Sanity: the complete value deserializes, so only the removal below
        // can be what makes the next assertion fail.
        assert_eq!(
            serde_json::from_value::<YearToDateContext>(fields.clone()).unwrap(),
            ytd
        );

        fields.as_object_mut().unwrap().remove("prior_employment");
        assert!(serde_json::from_value::<YearToDateContext>(fields).is_err());
    }

    #[test]
    fn deserialize_round_trips() {
        let ytd = YearToDateContext::new(
            TaxYear::starting(2026),
            Money::from_cents(100).unwrap(),
            Money::from_cents(10).unwrap(),
            PeriodsElapsed::new(3).unwrap(),
            PriorEmployment::None,
        );
        let json = serde_json::to_string(&ytd).unwrap();
        assert_eq!(
            serde_json::from_str::<YearToDateContext>(&json).unwrap(),
            ytd
        );
    }
}
