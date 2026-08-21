//! The payroll calculator: `calculate(&PayrollInput, &PayrollRules,
//! PaySchedule) -> Result<PayrollCalculation, PayrollError>`.
//!
//! A plain function, no trait: it stays a concrete function until a second
//! implementation genuinely exists. Proration, band application, the SSC
//! clamp, and rounding have no exported surface of their own — they are
//! verified only through the values `calculate` returns.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::deduction::{Deduction, StatutoryDeduction};
use crate::earning::{Earning, RemunerationBases};
use crate::employment::EmploymentSnapshot;
use crate::money::{Money, MoneyError};
use crate::pay_period::PayPeriod;
use crate::pay_schedule::PaySchedule;
use crate::rules::{BandContribution, PayrollRules, SscClamp};
use crate::year_to_date::{PeriodsElapsed, YearToDateContext};

/// The complete, self-contained set of facts one calculation needs. If it
/// is not in the `PayrollInput`, the calculator cannot see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayrollInput {
    employment: EmploymentSnapshot,
    period: PayPeriod,
    /// Earning lines beyond `BasicPay` — allowances. `calculate` adds the
    /// `BasicPay` line itself from the Employment's `CompensationTerms` — a
    /// caller cannot supply a second one here.
    earnings: Vec<Earning>,
    year_to_date: YearToDateContext,
}

impl PayrollInput {
    pub fn new(
        employment: EmploymentSnapshot,
        period: PayPeriod,
        earnings: Vec<Earning>,
        year_to_date: YearToDateContext,
    ) -> Self {
        PayrollInput {
            employment,
            period,
            earnings,
            year_to_date,
        }
    }
}

/// Why a `calculate` call was refused. Refusals carry the same weight as
/// the arithmetic (INV-012): Salt never guesses at a payroll situation it
/// has no rule for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayrollError {
    /// The Employment's `CompensationTerms` do not cover the whole
    /// `PayPeriod` being calculated.
    CompensationTermsDoNotCoverPeriod,
    /// `CompensationTerms.EffectiveFrom` is not itself a `PayPeriod` start
    /// date for the Employer's `PaySchedule` (INV-014). A pay rise dated
    /// mid-period is refused rather than silently rounded onto the next
    /// period — an Employer must never believe a rise took effect on the
    /// 15th while Salt quietly disagrees.
    CompensationTermsNotEffectiveOnAPeriodStart {
        next_valid_effective_from: NaiveDate,
    },
    /// The Employment's end date is before its start date.
    ContradictoryEmploymentDates,
    /// The Employment does not overlap the `PayPeriod` at all. A joiner or
    /// leaver — the Employment starting or ending inside the period — is
    /// not this error; it is prorated instead (§8.2).
    EmploymentDoesNotOverlapPeriod,
    /// `PayrollInput.earnings` contained a `BasicPay` line. `calculate`
    /// derives that line itself from the Employment's `CompensationTerms`,
    /// which is also the social security base — a second one supplied here
    /// would silently change both gross and that base, so it is refused
    /// rather than added (INV-012).
    DuplicateBasicPayLine,
    /// Recalculating cumulative PAYE against the corrected year-to-date
    /// figures produced a liability lower than what the context says was
    /// already withheld. Refund handling is not modeled anywhere in this
    /// domain yet, so this is refused rather than silently clamped to zero
    /// or turned into a negative deduction (INV-012).
    PriorPayeExceedsRecalculatedLiability,
    /// PAYE plus employee social security exceeded gross remuneration.
    DeductionsExceedGrossRemuneration,
    /// A monetary amount overflowed `i64` cents during calculation.
    AmountOverflow,
}

impl std::fmt::Display for PayrollError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PayrollError::CompensationTermsDoNotCoverPeriod => {
                write!(f, "compensation terms do not cover the pay period")
            }
            PayrollError::CompensationTermsNotEffectiveOnAPeriodStart {
                next_valid_effective_from,
            } => {
                write!(
                    f,
                    "compensation terms effective_from must be a pay period start date; the next valid effective date is {next_valid_effective_from}"
                )
            }
            PayrollError::ContradictoryEmploymentDates => {
                write!(f, "employment end date is before its start date")
            }
            PayrollError::EmploymentDoesNotOverlapPeriod => {
                write!(f, "employment does not overlap the pay period")
            }
            PayrollError::DuplicateBasicPayLine => {
                write!(
                    f,
                    "BasicPay is derived by calculate() from the compensation terms and cannot also be supplied in PayrollInput.earnings"
                )
            }
            PayrollError::PriorPayeExceedsRecalculatedLiability => {
                write!(f, "prior PAYE exceeds recalculated year-to-date liability")
            }
            PayrollError::DeductionsExceedGrossRemuneration => {
                write!(f, "deductions exceed gross remuneration")
            }
            PayrollError::AmountOverflow => write!(f, "a monetary amount overflowed"),
        }
    }
}

impl std::error::Error for PayrollError {}

/// Every `MoneyError` reachable from inside `calculate` is an overflow:
/// the amounts being combined are already validated non-negative, and
/// every value is built through `Money`, never from a raw decimal that
/// could carry fractional cents.
impl From<MoneyError> for PayrollError {
    fn from(_: MoneyError) -> PayrollError {
        PayrollError::AmountOverflow
    }
}

/// A condition Salt flags without blocking calculation. This ticket
/// produces none; `warnings` is always empty until a later ticket
/// introduces the first variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Warning {}

/// The year-to-date figures PAYE was derived from, for explainability. No
/// free-text formula strings — structured data only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayeTrace {
    pub prior_taxable_remuneration: Money,
    pub prior_paye: Money,
    pub this_period_taxable_remuneration: Money,
    pub year_to_date_taxable_remuneration: Money,
    /// The exact, unrounded tax owed on `year_to_date_taxable_remuneration`
    /// under the period-scaled annual bands. Intermediate arithmetic is
    /// never rounded, so unlike every `Money` figure here this one is not
    /// cents-exact.
    pub year_to_date_tax_owed: Decimal,
    /// Which PAYE bands were crossed and how much each contributed.
    pub bands_applied: Vec<BandContribution>,
    pub periods_elapsed: PeriodsElapsed,
}

/// The base, rate, and any floor or ceiling applied to one social security
/// figure, for explainability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SscTrace {
    pub basic_pay: Money,
    /// `basic_pay` clamped to `floor`/`ceiling` — the actual base charged.
    pub base: Money,
    pub clamp: SscClamp,
    pub rate: Decimal,
    pub floor: Money,
    pub ceiling: Money,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayeResult {
    pub amount: Money,
    pub trace: PayeTrace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SscResult {
    pub amount: Money,
    pub trace: SscTrace,
}

/// The result of calculating one Employment for one PayPeriod.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayrollCalculation {
    pub earning_lines: Vec<Earning>,
    pub gross_remuneration: Money,
    pub taxable_remuneration: Money,
    pub paye: PayeResult,
    pub employee_social_security: SscResult,
    /// The Employer's own cost. Never appears in `deductions` and never
    /// reduces `net_pay` (INV-007).
    pub employer_social_security: SscResult,
    /// `PAYE` and employee social security only — matches
    /// `gross_remuneration - deductions == net_pay`.
    pub deductions: Vec<Deduction>,
    pub net_pay: Money,
    pub warnings: Vec<Warning>,
}

pub fn calculate(
    input: &PayrollInput,
    rules: &PayrollRules,
    schedule: PaySchedule,
) -> Result<PayrollCalculation, PayrollError> {
    if !input.employment.has_coherent_dates() {
        return Err(PayrollError::ContradictoryEmploymentDates);
    }
    let employed_days = input
        .employment
        .overlap_days(input.period)
        .ok_or(PayrollError::EmploymentDoesNotOverlapPeriod)?;
    let period_days = (input.period.end() - input.period.start()).num_days() + 1;

    let terms = input.employment.compensation_terms();
    if !schedule.is_period_start(terms.effective_from()) {
        // Unreachable in practice: the only way to fail here is a calendar
        // edge so extreme `next_period_start_after` cannot name a date
        // within the representable range.
        let next_valid_effective_from = schedule
            .next_period_start_after(terms.effective_from())
            .ok_or(PayrollError::AmountOverflow)?;
        return Err(PayrollError::CompensationTermsNotEffectiveOnAPeriodStart {
            next_valid_effective_from,
        });
    }
    if !terms.covers(input.period) {
        return Err(PayrollError::CompensationTermsDoNotCoverPeriod);
    }
    if input
        .earnings
        .iter()
        .any(|earning| matches!(earning, Earning::BasicPay(_)))
    {
        return Err(PayrollError::DuplicateBasicPayLine);
    }

    // Proration (§8.2) applies to BasicPay only, and only for a joiner or
    // leaver — `employed_days < period_days`. A continuing employee's
    // BasicPay is carried through untouched, never round-tripped through
    // decimal division, so twelve full periods sum to exactly twelve
    // months' pay with no rounding drift.
    let basic_pay = if employed_days < period_days {
        let unrounded = terms
            .basic_pay()
            .as_decimal()
            .checked_mul(Decimal::from(employed_days))
            .ok_or(PayrollError::AmountOverflow)?
            .checked_div(Decimal::from(period_days))
            .ok_or(PayrollError::AmountOverflow)?;
        rules.rounding_rule().apply(unrounded)?
    } else {
        terms.basic_pay()
    };

    // Every allowance the caller supplied is kept as its own line, in the
    // order given, after the derived `BasicPay` line. Lines of the same
    // kind are never merged: a payslip has to be able to show each one.
    let mut earning_lines = Vec::with_capacity(input.earnings.len() + 1);
    earning_lines.push(Earning::BasicPay(basic_pay));
    earning_lines.extend(input.earnings.iter().copied());

    // Gross, taxable, and the social security base are accumulated
    // separately from the same lines — never one summation filtered three
    // ways. See `RemunerationBases`.
    let bases = RemunerationBases::accumulate(earning_lines.iter())?;
    let gross_remuneration = bases.gross();
    let taxable_remuneration = bases.taxable();

    let ytd = input.year_to_date;
    let year_to_date_taxable_remuneration = ytd
        .prior_taxable_remuneration()
        .checked_add(taxable_remuneration)?;
    let (year_to_date_tax_owed, bands_applied) = rules.tax_owed_on(
        year_to_date_taxable_remuneration.as_decimal(),
        ytd.periods_elapsed().period_number(),
    )?;
    let paye_unrounded = year_to_date_tax_owed
        .checked_sub(ytd.prior_paye().as_decimal())
        .ok_or(PayrollError::AmountOverflow)?;
    // Compared against zero rather than tested for sign, because an exact
    // decimal can carry a negative sign on a zero value.
    if paye_unrounded < Decimal::ZERO {
        return Err(PayrollError::PriorPayeExceedsRecalculatedLiability);
    }
    let paye_amount = rules.rounding_rule().apply(paye_unrounded)?;

    let social_security = rules.social_security();
    // Taken from the accumulated bases, not re-read from the compensation
    // terms: the base an allowance must not reach is the same number the
    // `BasicPay` line put into gross, and one source keeps it that way.
    let basic_pay = bases.social_security();
    let (ssc_base, ssc_clamp) = social_security.base(basic_pay);

    let contribution = |rate: Decimal| -> Result<Money, PayrollError> {
        let unrounded = ssc_base
            .as_decimal()
            .checked_mul(rate)
            .ok_or(PayrollError::AmountOverflow)?;
        Ok(rules.rounding_rule().apply(unrounded)?)
    };
    let employee_ssc_amount = contribution(social_security.employee_rate())?;
    let employer_ssc_amount = contribution(social_security.employer_rate())?;

    let deductions = vec![
        Deduction::Statutory(StatutoryDeduction::PAYE(paye_amount)),
        Deduction::Statutory(StatutoryDeduction::SocialSecurity(employee_ssc_amount)),
    ];

    // Both `Money` amounts are already non-negative, so the only way this
    // subtraction fails is by going below zero — i.e. the deductions
    // exceeded gross remuneration.
    let net_pay = gross_remuneration
        .checked_sub(paye_amount)
        .and_then(|remainder| remainder.checked_sub(employee_ssc_amount))
        .map_err(|_| PayrollError::DeductionsExceedGrossRemuneration)?;

    Ok(PayrollCalculation {
        earning_lines,
        gross_remuneration,
        taxable_remuneration,
        paye: PayeResult {
            amount: paye_amount,
            trace: PayeTrace {
                prior_taxable_remuneration: ytd.prior_taxable_remuneration(),
                prior_paye: ytd.prior_paye(),
                this_period_taxable_remuneration: taxable_remuneration,
                year_to_date_taxable_remuneration,
                year_to_date_tax_owed,
                bands_applied,
                periods_elapsed: ytd.periods_elapsed(),
            },
        },
        employee_social_security: SscResult {
            amount: employee_ssc_amount,
            trace: SscTrace {
                basic_pay,
                base: ssc_base,
                clamp: ssc_clamp,
                rate: social_security.employee_rate(),
                floor: social_security.floor(),
                ceiling: social_security.ceiling(),
            },
        },
        employer_social_security: SscResult {
            amount: employer_ssc_amount,
            trace: SscTrace {
                basic_pay,
                base: ssc_base,
                clamp: ssc_clamp,
                rate: social_security.employer_rate(),
                floor: social_security.floor(),
                ceiling: social_security.ceiling(),
            },
        },
        deductions,
        net_pay,
        warnings: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::employment::{
        CompensationTerms, EmployerId, EmploymentId, PersonId, PersonReference,
    };
    use crate::pay_schedule::PeriodEndDay;
    use crate::rules::{PayeBand, RoundingRule, SocialSecurityRules};
    use crate::tax_year::TaxYear;
    use crate::year_to_date::PeriodsElapsed;
    use chrono::NaiveDate;
    use rust_decimal_macros::dec;

    fn money(amount: Decimal) -> Money {
        Money::from_decimal(amount).unwrap()
    }

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    /// Thresholds are multiples of 12 so that scaling by
    /// periods_elapsed_inclusive/12 stays exact decimal arithmetic:
    /// 0-120,000: 0%, 120,000-240,000: 20%, 240,000-480,000: 30%,
    /// 480,000+: 40%. SSC 0.9% each way, floor N$500, ceiling N$11,000 —
    /// synthetic test figures, not a statutory table (that arrives with
    /// `ruleset_for` in a later ticket).
    fn test_rules() -> PayrollRules {
        let bands = vec![
            PayeBand::new(money(dec!(0)), dec!(0.00)).unwrap(),
            PayeBand::new(money(dec!(120000)), dec!(0.20)).unwrap(),
            PayeBand::new(money(dec!(240000)), dec!(0.30)).unwrap(),
            PayeBand::new(money(dec!(480000)), dec!(0.40)).unwrap(),
        ];
        let social_security = SocialSecurityRules::new(
            dec!(0.009),
            dec!(0.009),
            money(dec!(500)),
            money(dec!(11000)),
        )
        .unwrap();
        PayrollRules::new(bands, social_security, RoundingRule::HalfUpToCents).unwrap()
    }

    fn test_period() -> PayPeriod {
        PayPeriod::new(date(2026, 1, 26), date(2026, 2, 25)).unwrap()
    }

    /// A calendar-month schedule, chosen only so `terms()`'s `2025-01-01`
    /// `effective_from` is a valid period start (INV-014) — it is not
    /// meant to describe `test_period()`'s own 26th-to-25th cycle. Tests
    /// that exercise INV-014 itself build their own period-matched
    /// schedule.
    fn test_schedule() -> PaySchedule {
        PaySchedule::new(PeriodEndDay::LastDayOfMonth)
    }

    fn test_tax_year() -> TaxYear {
        TaxYear::starting(2025)
    }

    fn snapshot(
        start_date: NaiveDate,
        end_date: Option<NaiveDate>,
        terms: CompensationTerms,
    ) -> EmploymentSnapshot {
        EmploymentSnapshot::new(
            EmploymentId::new("emp-1"),
            EmployerId::new("employer-1"),
            PersonReference::new(PersonId::new("person-1")),
            start_date,
            end_date,
            terms,
        )
    }

    fn employment_paying(basic_pay: Decimal) -> EmploymentSnapshot {
        let terms = CompensationTerms::new(date(2025, 1, 1), None, money(basic_pay)).unwrap();
        snapshot(date(2025, 1, 1), None, terms)
    }

    fn input_for(basic_pay: Decimal, ytd: YearToDateContext) -> PayrollInput {
        PayrollInput::new(employment_paying(basic_pay), test_period(), Vec::new(), ytd)
    }

    fn ytd(prior_taxable: Decimal, prior_paye: Decimal, periods_elapsed: u8) -> YearToDateContext {
        YearToDateContext::new(
            test_tax_year(),
            money(prior_taxable),
            money(prior_paye),
            PeriodsElapsed::new(periods_elapsed).unwrap(),
        )
    }

    /// Asserted across every scenario: gross less all deductions equals net
    /// pay, employer SSC never appears in the deduction total (INV-007),
    /// and the returned earning lines independently reproduce all three
    /// bases (INV-006).
    ///
    /// The three totals are recomputed here by a separate exhaustive match
    /// rather than by calling the production accumulator, so a wrong
    /// classification cannot agree with itself.
    fn assert_invariants(calc: &PayrollCalculation) {
        let (mut ssc_base, mut taxable, mut gross) = (Money::ZERO, Money::ZERO, Money::ZERO);
        let mut basic_pay_lines = 0;
        for line in &calc.earning_lines {
            match *line {
                Earning::BasicPay(amount) => {
                    basic_pay_lines += 1;
                    ssc_base = ssc_base.checked_add(amount).unwrap();
                    taxable = taxable.checked_add(amount).unwrap();
                    gross = gross.checked_add(amount).unwrap();
                }
                Earning::TaxableAllowance(amount) => {
                    taxable = taxable.checked_add(amount).unwrap();
                    gross = gross.checked_add(amount).unwrap();
                }
                Earning::NonTaxableAllowance(amount) => {
                    gross = gross.checked_add(amount).unwrap();
                }
            }
        }
        assert_eq!(
            basic_pay_lines, 1,
            "exactly one BasicPay line must be returned"
        );
        assert!(
            matches!(calc.earning_lines.first(), Some(Earning::BasicPay(_))),
            "the BasicPay line must come first"
        );
        assert_eq!(
            gross, calc.gross_remuneration,
            "gross must be the total of every earning line"
        );
        assert_eq!(
            taxable, calc.taxable_remuneration,
            "taxable must be BasicPay plus TaxableAllowance only"
        );
        assert_eq!(
            ssc_base, calc.employee_social_security.trace.basic_pay,
            "the social security base must come from BasicPay alone"
        );
        assert_eq!(
            calc.employee_social_security.trace.basic_pay,
            calc.employer_social_security.trace.basic_pay,
            "both social security figures must share one base"
        );
        assert_eq!(
            calc.taxable_remuneration, calc.paye.trace.this_period_taxable_remuneration,
            "the PAYE trace must state the taxable figure PAYE was derived from"
        );

        let deduction_total =
            Money::checked_sum(calc.deductions.iter().map(|d| d.amount())).unwrap();
        assert_eq!(
            calc.gross_remuneration
                .checked_sub(deduction_total)
                .unwrap(),
            calc.net_pay,
            "gross less deductions must equal net pay"
        );
        assert_eq!(
            deduction_total,
            calc.paye
                .amount
                .checked_add(calc.employee_social_security.amount)
                .unwrap(),
            "employer SSC must never appear in the deduction total"
        );
    }

    // PC-001: ordinary monthly salaried employee (baseline). Period 12 of
    // the tax year, so the PAYE bands are unscaled.
    #[test]
    fn pc_001_ordinary_monthly_salaried_employee() {
        let input = input_for(dec!(15000.00), ytd(dec!(110000.00), dec!(0.00), 11));
        let calc = calculate(&input, &test_rules(), test_schedule()).unwrap();

        assert_eq!(calc.gross_remuneration, money(dec!(15000.00)));
        assert_eq!(calc.taxable_remuneration, money(dec!(15000.00)));
        assert_eq!(calc.paye.amount, money(dec!(1000.00)));
        assert_eq!(calc.employee_social_security.amount, money(dec!(99.00)));
        assert_eq!(calc.employer_social_security.amount, money(dec!(99.00)));
        assert_eq!(calc.net_pay, money(dec!(13901.00)));
        assert_invariants(&calc);
    }

    // PC-002: employee below the PAYE threshold — zero-PAYE path.
    #[test]
    fn pc_002_below_the_paye_threshold() {
        let input = input_for(
            dec!(5000.00),
            YearToDateContext::first_period(test_tax_year()),
        );
        let calc = calculate(&input, &test_rules(), test_schedule()).unwrap();

        assert_eq!(calc.paye.amount, Money::ZERO);
        assert_eq!(calc.employee_social_security.amount, money(dec!(45.00)));
        assert_eq!(calc.net_pay, money(dec!(4955.00)));
        assert_invariants(&calc);
    }

    // PC-003: employee crossing a tax bracket — progressive band logic.
    #[test]
    fn pc_003_crossing_a_tax_bracket() {
        let input = input_for(dec!(15000.00), ytd(dec!(19000.00), dec!(0.00), 2));
        let calc = calculate(&input, &test_rules(), test_schedule()).unwrap();

        assert_eq!(calc.paye.amount, money(dec!(800.00)));
        assert_eq!(calc.employee_social_security.amount, money(dec!(99.00)));
        assert_eq!(calc.net_pay, money(dec!(14101.00)));

        // Period 3's bands are scaled to 3/12 of their annual thresholds:
        // the (zero-tax) first band up to 30,000, then the second band's
        // 20% picks up the remaining 4,000 of the 34,000 year-to-date
        // taxable amount.
        let bands = &calc.paye.trace.bands_applied;
        assert_eq!(bands.len(), 2);
        assert_eq!(bands[0].threshold, dec!(0));
        assert_eq!(bands[0].tax, dec!(0));
        assert_eq!(bands[1].threshold, dec!(30000));
        assert_eq!(bands[1].rate, dec!(0.20));
        assert_eq!(bands[1].tax, dec!(800.00));

        assert_invariants(&calc);
    }

    // PC-004: employee crossing multiple brackets — higher-range logic.
    #[test]
    fn pc_004_crossing_multiple_brackets() {
        let input = input_for(dec!(180000.00), ytd(dec!(150000.00), dec!(25000.00), 5));
        let calc = calculate(&input, &test_rules(), test_schedule()).unwrap();

        assert_eq!(calc.paye.amount, money(dec!(59000.00)));
        assert_eq!(calc.employee_social_security.amount, money(dec!(99.00)));
        assert_eq!(calc.net_pay, money(dec!(120901.00)));
        assert_invariants(&calc);
    }

    // PC-009: BasicPay above the SSC ceiling — ceiling clamp.
    #[test]
    fn pc_009_above_the_ssc_ceiling() {
        let input = input_for(
            dec!(20000.00),
            YearToDateContext::first_period(test_tax_year()),
        );
        let calc = calculate(&input, &test_rules(), test_schedule()).unwrap();

        assert_eq!(calc.paye.amount, money(dec!(2000.00)));
        assert_eq!(calc.employee_social_security.amount, money(dec!(99.00)));
        assert_eq!(
            calc.employee_social_security.trace.base,
            money(dec!(11000.00))
        );
        assert_eq!(calc.employee_social_security.trace.clamp, SscClamp::Ceiling);
        assert_eq!(calc.net_pay, money(dec!(17901.00)));
        assert_invariants(&calc);
    }

    // PC-010: BasicPay below the SSC floor — floor clamp.
    #[test]
    fn pc_010_below_the_ssc_floor() {
        let input = input_for(
            dec!(300.00),
            YearToDateContext::first_period(test_tax_year()),
        );
        let calc = calculate(&input, &test_rules(), test_schedule()).unwrap();

        assert_eq!(calc.employee_social_security.amount, money(dec!(4.50)));
        assert_eq!(
            calc.employee_social_security.trace.base,
            money(dec!(500.00))
        );
        assert_eq!(calc.employee_social_security.trace.clamp, SscClamp::Floor);
        assert_eq!(calc.net_pay, money(dec!(295.50)));
        assert_invariants(&calc);
    }

    // PC-011: mid-year adoption with an OpeningBalance — cumulative PAYE
    // from prior totals entered at adoption, not accumulated by Salt.
    #[test]
    fn pc_011_mid_year_adoption_with_opening_balance() {
        let input = input_for(dec!(100000.00), ytd(dec!(200000.00), dec!(32000.00), 7));
        let calc = calculate(&input, &test_rules(), test_schedule()).unwrap();

        assert_eq!(calc.paye.amount, money(dec!(26000.00)));
        assert_eq!(calc.net_pay, money(dec!(73901.00)));
        assert_invariants(&calc);
    }

    // PC-012: second period of a tax year — PAYE net of prior withholding.
    #[test]
    fn pc_012_second_period_of_a_tax_year() {
        let input = input_for(dec!(20000.00), ytd(dec!(15000.00), dec!(1000.00), 1));
        let calc = calculate(&input, &test_rules(), test_schedule()).unwrap();

        assert_eq!(calc.paye.amount, money(dec!(2000.00)));
        assert_eq!(calc.net_pay, money(dec!(17901.00)));

        // The trace carries PAYE already withheld this tax year and the
        // year-to-date liability it was subtracted from, as typed values —
        // this period's PAYE is the difference, and is `calc.paye.amount`.
        assert_eq!(calc.paye.trace.prior_paye, money(dec!(1000.00)));
        assert_eq!(
            calc.paye.trace.prior_taxable_remuneration,
            money(dec!(15000.00))
        );
        assert_eq!(
            calc.paye.trace.year_to_date_taxable_remuneration,
            money(dec!(35000.00))
        );
        assert_eq!(calc.paye.trace.year_to_date_tax_owed, dec!(3000.00));
        assert_eq!(calc.paye.trace.periods_elapsed.get(), 1);

        assert_invariants(&calc);
    }

    // PC-015: a corrected earlier period, absorbed forward. The YTD context
    // already reflects the correction — calculate() has no special-cased
    // "correction" path; it is simply given the fixed totals and continues
    // as normal (ADR-0002).
    #[test]
    fn pc_015_corrected_earlier_period_absorbed_forward() {
        let input = input_for(dec!(25000.00), ytd(dec!(45000.00), dec!(3000.00), 3));
        let calc = calculate(&input, &test_rules(), test_schedule()).unwrap();

        assert_eq!(calc.paye.amount, money(dec!(3000.00)));
        assert_eq!(calc.net_pay, money(dec!(21901.00)));
        assert_invariants(&calc);
    }

    #[test]
    fn refuses_contradictory_employment_dates() {
        let terms = CompensationTerms::new(date(2025, 1, 1), None, money(dec!(5000.00))).unwrap();
        let employment = snapshot(date(2026, 3, 1), Some(date(2026, 1, 1)), terms);
        let input = PayrollInput::new(
            employment,
            test_period(),
            Vec::new(),
            YearToDateContext::first_period(test_tax_year()),
        );

        assert_eq!(
            calculate(&input, &test_rules(), test_schedule()),
            Err(PayrollError::ContradictoryEmploymentDates)
        );
    }

    #[test]
    fn refuses_compensation_terms_that_do_not_cover_the_period() {
        // Effective only from after the period starts.
        let terms = CompensationTerms::new(date(2026, 2, 1), None, money(dec!(5000.00))).unwrap();
        let employment = snapshot(date(2025, 1, 1), None, terms);
        let input = PayrollInput::new(
            employment,
            test_period(),
            Vec::new(),
            YearToDateContext::first_period(test_tax_year()),
        );

        assert_eq!(
            calculate(&input, &test_rules(), test_schedule()),
            Err(PayrollError::CompensationTermsDoNotCoverPeriod)
        );
    }

    // PC-005: employment starts mid-period — joiner proration. A 31-day
    // calendar-month period (Jan 2026); BasicPay 9,300.00/month is
    // 300.00/day. The employee joins Jan 22, so only Jan 22-31 (10 days)
    // is worked: 300.00 x 10 = 3,000.00, well under the first period's
    // scaled zero-tax band (120,000/12 = 10,000), so PAYE is zero.
    #[test]
    fn pc_005_employment_starts_mid_period_prorates_basic_pay() {
        let period = PayPeriod::new(date(2026, 1, 1), date(2026, 1, 31)).unwrap();
        let terms = CompensationTerms::new(date(2026, 1, 1), None, money(dec!(9300.00))).unwrap();
        let employment = snapshot(date(2026, 1, 22), None, terms);
        let input = PayrollInput::new(
            employment,
            period,
            Vec::new(),
            YearToDateContext::first_period(test_tax_year()),
        );
        let calc = calculate(&input, &test_rules(), test_schedule()).unwrap();

        assert_eq!(
            calc.earning_lines,
            vec![Earning::BasicPay(money(dec!(3000.00)))]
        );
        assert_eq!(calc.gross_remuneration, money(dec!(3000.00)));
        assert_eq!(calc.taxable_remuneration, money(dec!(3000.00)));
        assert_eq!(calc.paye.amount, Money::ZERO);
        assert_eq!(calc.employee_social_security.amount, money(dec!(27.00)));
        assert_eq!(calc.net_pay, money(dec!(2973.00)));
        assert_invariants(&calc);
    }

    // PC-006: employment ends mid-period — leaver proration. A 28-day
    // calendar-month period (Feb 2026, not a leap year); BasicPay
    // 8,400.00/month is 300.00/day. The employee leaves Feb 12, so only
    // Feb 1-12 (12 days) is worked: 300.00 x 12 = 3,600.00, again under
    // the zero-tax band, so PAYE is zero.
    #[test]
    fn pc_006_employment_ends_mid_period_prorates_basic_pay() {
        let period = PayPeriod::new(date(2026, 2, 1), date(2026, 2, 28)).unwrap();
        let terms = CompensationTerms::new(date(2026, 2, 1), None, money(dec!(8400.00))).unwrap();
        let employment = snapshot(date(2025, 1, 1), Some(date(2026, 2, 12)), terms);
        let input = PayrollInput::new(
            employment,
            period,
            Vec::new(),
            YearToDateContext::first_period(test_tax_year()),
        );
        let calc = calculate(&input, &test_rules(), test_schedule()).unwrap();

        assert_eq!(
            calc.earning_lines,
            vec![Earning::BasicPay(money(dec!(3600.00)))]
        );
        assert_eq!(calc.gross_remuneration, money(dec!(3600.00)));
        assert_eq!(calc.taxable_remuneration, money(dec!(3600.00)));
        assert_eq!(calc.paye.amount, Money::ZERO);
        assert_eq!(calc.employee_social_security.amount, money(dec!(32.40)));
        assert_eq!(calc.net_pay, money(dec!(3567.60)));
        assert_invariants(&calc);
    }

    // Proration applies to BasicPay only: a joiner's allowance is paid in
    // full even though BasicPay is cut down to the days worked.
    #[test]
    fn proration_never_touches_an_allowance() {
        let period = PayPeriod::new(date(2026, 1, 1), date(2026, 1, 31)).unwrap();
        let terms = CompensationTerms::new(date(2026, 1, 1), None, money(dec!(9300.00))).unwrap();
        let employment = snapshot(date(2026, 1, 22), None, terms);
        let input = PayrollInput::new(
            employment,
            period,
            vec![Earning::NonTaxableAllowance(money(dec!(500.00)))],
            YearToDateContext::first_period(test_tax_year()),
        );
        let calc = calculate(&input, &test_rules(), test_schedule()).unwrap();

        assert_eq!(
            calc.earning_lines,
            vec![
                Earning::BasicPay(money(dec!(3000.00))),
                Earning::NonTaxableAllowance(money(dec!(500.00))),
            ]
        );
        assert_invariants(&calc);
    }

    // Twelve consecutive unprorated periods must sum to exactly twelve
    // months' pay. BasicPay deliberately does not divide evenly by any
    // period's day count, so a calculator that always divided by
    // `period_days` — even for a continuing employee — would accumulate
    // rounding drift here; skipping division when the period is fully
    // covered is what keeps the total exact.
    #[test]
    fn twelve_consecutive_full_periods_sum_to_exactly_twelve_months_pay() {
        let schedule = test_schedule();
        let periods = schedule
            .generate_periods(2026, crate::pay_schedule::Month::new(1).unwrap(), 12)
            .unwrap();
        let basic_pay = dec!(12345.67);
        let terms = CompensationTerms::new(periods[0].start(), None, money(basic_pay)).unwrap();

        let mut total = Money::ZERO;
        for period in &periods {
            let employment = snapshot(date(2020, 1, 1), None, terms);
            let input = PayrollInput::new(
                employment,
                *period,
                Vec::new(),
                YearToDateContext::first_period(test_tax_year()),
            );
            let calc = calculate(&input, &test_rules(), schedule).unwrap();
            assert_eq!(
                calc.earning_lines,
                vec![Earning::BasicPay(money(basic_pay))]
            );
            total = total.checked_add(calc.earning_lines[0].amount()).unwrap();
        }

        assert_eq!(total, money(basic_pay * dec!(12)));
    }

    #[test]
    fn refuses_an_employment_that_does_not_overlap_the_period_at_all() {
        let terms = CompensationTerms::new(date(2025, 1, 1), None, money(dec!(5000.00))).unwrap();
        // The employment ended well before test_period() (2026-01-26 to
        // 2026-02-25) begins — a genuine mismatch, not a leaver.
        let employment = snapshot(date(2025, 1, 1), Some(date(2025, 12, 1)), terms);
        let input = PayrollInput::new(
            employment,
            test_period(),
            Vec::new(),
            YearToDateContext::first_period(test_tax_year()),
        );

        assert_eq!(
            calculate(&input, &test_rules(), test_schedule()),
            Err(PayrollError::EmploymentDoesNotOverlapPeriod)
        );
    }

    // INV-014: a `CompensationTerms.EffectiveFrom` that is not itself a
    // `PayPeriod` start date is refused — this is the "pay rise dated
    // mid-period" case, distinct from a joiner or leaver. The schedule
    // here matches test_period()'s own 26th-to-25th cycle, so the next
    // valid date is 2026-01-26 — test_period()'s own start.
    #[test]
    fn refuses_compensation_terms_not_effective_on_a_period_start() {
        let schedule = PaySchedule::new(PeriodEndDay::Day(
            crate::pay_schedule::DayOfMonth::new(25).unwrap(),
        ));
        // 2026-01-10 is before test_period()'s start (so the terms still
        // cover the period) but is not itself a period start.
        let terms = CompensationTerms::new(date(2026, 1, 10), None, money(dec!(5000.00))).unwrap();
        let employment = snapshot(date(2025, 1, 1), None, terms);
        let input = PayrollInput::new(
            employment,
            test_period(),
            Vec::new(),
            YearToDateContext::first_period(test_tax_year()),
        );

        assert_eq!(
            calculate(&input, &test_rules(), schedule),
            Err(PayrollError::CompensationTermsNotEffectiveOnAPeriodStart {
                next_valid_effective_from: date(2026, 1, 26),
            })
        );
    }

    // PC-007: taxable allowance — classification affects PAYE, not SSC.
    // BasicPay 15,000.00 + TaxableAllowance 2,000.00, period 12 (bands
    // unscaled). Year-to-date taxable becomes 110,000 + 17,000 = 127,000:
    // the first 120,000 is untaxed, the remaining 7,000 at 20% is 1,400.00.
    // SSC is charged on BasicPay alone, so it is unchanged from PC-001
    // despite the extra 2,000 of remuneration.
    #[test]
    fn pc_007_taxable_allowance_affects_paye_not_ssc() {
        let input = PayrollInput::new(
            employment_paying(dec!(15000.00)),
            test_period(),
            vec![Earning::TaxableAllowance(money(dec!(2000.00)))],
            ytd(dec!(110000.00), dec!(0.00), 11),
        );
        let calc = calculate(&input, &test_rules(), test_schedule()).unwrap();

        assert_eq!(calc.gross_remuneration, money(dec!(17000.00)));
        assert_eq!(calc.taxable_remuneration, money(dec!(17000.00)));
        assert_eq!(calc.paye.amount, money(dec!(1400.00)));
        assert_eq!(calc.employee_social_security.amount, money(dec!(99.00)));
        assert_eq!(calc.net_pay, money(dec!(15501.00)));
        assert_eq!(
            calc.earning_lines,
            vec![
                Earning::BasicPay(money(dec!(15000.00))),
                Earning::TaxableAllowance(money(dec!(2000.00))),
            ]
        );
        // The allowance raised PAYE by 400.00 over PC-001 and left both
        // social security figures untouched.
        assert_eq!(
            calc.employee_social_security.trace.basic_pay,
            money(dec!(15000.00))
        );
        assert_eq!(calc.employer_social_security.amount, money(dec!(99.00)));
        assert_invariants(&calc);
    }

    // PC-008: non-taxable travel allowance — gross differs from taxable.
    // BasicPay 15,000.00 + NonTaxableAllowance 1,200.00: the allowance
    // swells gross to 16,200.00 but taxable stays 15,000.00, so PAYE and
    // SSC are identical to PC-001. Gross, taxable, and net all differ.
    #[test]
    fn pc_008_non_taxable_allowance_differs_gross_from_taxable() {
        let input = PayrollInput::new(
            employment_paying(dec!(15000.00)),
            test_period(),
            vec![Earning::NonTaxableAllowance(money(dec!(1200.00)))],
            ytd(dec!(110000.00), dec!(0.00), 11),
        );
        let calc = calculate(&input, &test_rules(), test_schedule()).unwrap();

        assert_eq!(calc.gross_remuneration, money(dec!(16200.00)));
        assert_eq!(calc.taxable_remuneration, money(dec!(15000.00)));
        assert_eq!(calc.paye.amount, money(dec!(1000.00)));
        assert_eq!(calc.employee_social_security.amount, money(dec!(99.00)));
        assert_eq!(calc.net_pay, money(dec!(15101.00)));
        assert_ne!(calc.gross_remuneration, calc.taxable_remuneration);
        assert_ne!(calc.taxable_remuneration, calc.net_pay);
        assert_ne!(calc.gross_remuneration, calc.net_pay);
        assert_eq!(
            calc.earning_lines,
            vec![
                Earning::BasicPay(money(dec!(15000.00))),
                Earning::NonTaxableAllowance(money(dec!(1200.00))),
            ]
        );
        assert_eq!(
            calc.employee_social_security.trace.basic_pay,
            money(dec!(15000.00))
        );
        assert_invariants(&calc);
    }

    // Both allowance kinds at once: the full three-way table in one
    // scenario. BasicPay 15,000.00 + TaxableAllowance 2,000.00 +
    // NonTaxableAllowance 1,200.00, period 12 (bands unscaled). Gross is
    // 18,200.00, taxable 17,000.00, and the social security base is the
    // 15,000.00 of BasicPay alone. Year-to-date taxable is 110,000 +
    // 17,000 = 127,000: 7,000 above the 120,000 threshold at 20% is
    // 1,400.00 of PAYE. Net is 18,200.00 - 1,400.00 - 99.00 = 16,701.00.
    #[test]
    fn both_allowance_kinds_give_three_different_totals() {
        let input = PayrollInput::new(
            employment_paying(dec!(15000.00)),
            test_period(),
            vec![
                Earning::TaxableAllowance(money(dec!(2000.00))),
                Earning::NonTaxableAllowance(money(dec!(1200.00))),
            ],
            ytd(dec!(110000.00), dec!(0.00), 11),
        );
        let calc = calculate(&input, &test_rules(), test_schedule()).unwrap();

        assert_eq!(calc.gross_remuneration, money(dec!(18200.00)));
        assert_eq!(calc.taxable_remuneration, money(dec!(17000.00)));
        assert_eq!(
            calc.employee_social_security.trace.basic_pay,
            money(dec!(15000.00))
        );
        assert_eq!(calc.paye.amount, money(dec!(1400.00)));
        assert_eq!(calc.employee_social_security.amount, money(dec!(99.00)));
        assert_eq!(calc.net_pay, money(dec!(16701.00)));

        // Gross, taxable, and net are three genuinely different numbers.
        assert_ne!(calc.gross_remuneration, calc.taxable_remuneration);
        assert_ne!(calc.taxable_remuneration, calc.net_pay);
        assert_ne!(calc.gross_remuneration, calc.net_pay);

        assert_invariants(&calc);
    }

    // A payslip renders the lines it is given, so each one is returned
    // separately and in the order supplied — two allowances of one kind
    // are never collapsed into a single line, even when their amounts are
    // equal.
    #[test]
    fn allowance_lines_are_returned_individually_in_order() {
        let input = PayrollInput::new(
            employment_paying(dec!(15000.00)),
            test_period(),
            vec![
                Earning::NonTaxableAllowance(money(dec!(600.00))),
                Earning::TaxableAllowance(money(dec!(600.00))),
                Earning::NonTaxableAllowance(money(dec!(600.00))),
            ],
            ytd(dec!(110000.00), dec!(0.00), 11),
        );
        let calc = calculate(&input, &test_rules(), test_schedule()).unwrap();

        assert_eq!(
            calc.earning_lines,
            vec![
                Earning::BasicPay(money(dec!(15000.00))),
                Earning::NonTaxableAllowance(money(dec!(600.00))),
                Earning::TaxableAllowance(money(dec!(600.00))),
                Earning::NonTaxableAllowance(money(dec!(600.00))),
            ]
        );
        assert_eq!(calc.gross_remuneration, money(dec!(16800.00)));
        assert_eq!(calc.taxable_remuneration, money(dec!(15600.00)));
        assert_invariants(&calc);
    }

    // A zero-amount allowance is a real line an employer may deliberately
    // record. It is kept and it changes nothing.
    #[test]
    fn a_zero_amount_allowance_is_kept_and_changes_nothing() {
        let with_zero = PayrollInput::new(
            employment_paying(dec!(15000.00)),
            test_period(),
            vec![Earning::TaxableAllowance(Money::ZERO)],
            ytd(dec!(110000.00), dec!(0.00), 11),
        );
        let calc = calculate(&with_zero, &test_rules(), test_schedule()).unwrap();
        let baseline = calculate(
            &input_for(dec!(15000.00), ytd(dec!(110000.00), dec!(0.00), 11)),
            &test_rules(),
            test_schedule(),
        )
        .unwrap();

        assert_eq!(calc.earning_lines.len(), 2);
        assert_eq!(calc.gross_remuneration, baseline.gross_remuneration);
        assert_eq!(calc.taxable_remuneration, baseline.taxable_remuneration);
        assert_eq!(calc.paye.amount, baseline.paye.amount);
        assert_eq!(calc.net_pay, baseline.net_pay);
        assert_invariants(&calc);
    }

    // An allowance large enough to overflow the cents total is refused,
    // not wrapped into a negative or nonsensical gross.
    #[test]
    fn refuses_an_allowance_that_overflows_the_total() {
        let input = PayrollInput::new(
            employment_paying(dec!(15000.00)),
            test_period(),
            vec![Earning::NonTaxableAllowance(
                Money::from_cents(i64::MAX).unwrap(),
            )],
            ytd(dec!(110000.00), dec!(0.00), 11),
        );

        assert_eq!(
            calculate(&input, &test_rules(), test_schedule()),
            Err(PayrollError::AmountOverflow)
        );
    }

    #[test]
    fn refuses_a_duplicate_basic_pay_line() {
        let input = PayrollInput::new(
            employment_paying(dec!(5000.00)),
            test_period(),
            vec![Earning::BasicPay(money(dec!(5000.00)))],
            YearToDateContext::first_period(test_tax_year()),
        );

        assert_eq!(
            calculate(&input, &test_rules(), test_schedule()),
            Err(PayrollError::DuplicateBasicPayLine)
        );
    }

    #[test]
    fn refuses_when_prior_paye_exceeds_recalculated_liability() {
        // A prior_paye figure inconsistent with any valid history: more tax
        // was supposedly already withheld than the recalculated
        // year-to-date liability can justify.
        let input = input_for(dec!(1000.00), ytd(dec!(1000.00), dec!(999999.00), 0));

        assert_eq!(
            calculate(&input, &test_rules(), test_schedule()),
            Err(PayrollError::PriorPayeExceedsRecalculatedLiability)
        );
    }

    #[test]
    fn refuses_when_deductions_would_exceed_gross_remuneration() {
        let bands = vec![PayeBand::new(money(dec!(0)), dec!(0.00)).unwrap()];
        // A deliberately pathological 200% employee rate to exercise the
        // refusal path; real rates are validated elsewhere to stay sane.
        let social_security =
            SocialSecurityRules::new(dec!(2.00), dec!(0.009), money(dec!(0)), money(dec!(11000)))
                .unwrap();
        let rules = PayrollRules::new(bands, social_security, RoundingRule::HalfUpToCents).unwrap();
        let input = input_for(
            dec!(5000.00),
            YearToDateContext::first_period(test_tax_year()),
        );

        assert_eq!(
            calculate(&input, &rules, test_schedule()),
            Err(PayrollError::DeductionsExceedGrossRemuneration)
        );
    }

    #[test]
    fn deserialize_round_trips() {
        let input = input_for(dec!(15000.00), ytd(dec!(110000.00), dec!(0.00), 11));
        let calc = calculate(&input, &test_rules(), test_schedule()).unwrap();

        let input_json = serde_json::to_string(&input).unwrap();
        assert_eq!(
            serde_json::from_str::<PayrollInput>(&input_json).unwrap(),
            input
        );

        let calc_json = serde_json::to_string(&calc).unwrap();
        assert_eq!(
            serde_json::from_str::<PayrollCalculation>(&calc_json).unwrap(),
            calc
        );

        let rules_json = serde_json::to_string(&test_rules()).unwrap();
        assert_eq!(
            serde_json::from_str::<PayrollRules>(&rules_json).unwrap(),
            test_rules()
        );
    }
}
