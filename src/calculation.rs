//! The payroll calculator: `calculate(&PayrollInput, &PayrollRules) ->
//! Result<PayrollCalculation, PayrollError>`.
//!
//! A plain function, no trait: it stays a concrete function until a second
//! implementation genuinely exists. Proration, band application, the SSC
//! clamp, and rounding have no exported surface of their own — they are
//! verified only through the values `calculate` returns.

use rust_decimal::Decimal;

use crate::deduction::{Deduction, StatutoryDeduction};
use crate::earning::Earning;
use crate::employment::EmploymentSnapshot;
use crate::money::Money;
use crate::pay_period::PayPeriod;
use crate::rules::PayrollRules;
use crate::year_to_date::YearToDateContext;

/// The complete, self-contained set of facts one calculation needs. If it
/// is not in the `PayrollInput`, the calculator cannot see it.
#[derive(Debug, Clone)]
pub struct PayrollInput {
    employment: EmploymentSnapshot,
    period: PayPeriod,
    /// Earning lines beyond `BasicPay` — allowances. `calculate` adds the
    /// `BasicPay` line itself from the Employment's `CompensationTerms`.
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
    /// The Employment's end date is before its start date.
    ContradictoryEmploymentDates,
}

/// The year-to-date figures PAYE was derived from, for explainability. No
/// free-text formula strings — structured data only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayeTrace {
    pub prior_taxable_remuneration: Money,
    pub prior_paye: Money,
    pub this_period_taxable_remuneration: Money,
    pub year_to_date_taxable_remuneration: Money,
    /// The exact, unrounded tax owed on `year_to_date_taxable_remuneration`
    /// under the annual bands. Intermediate arithmetic is never rounded, so
    /// unlike every `Money` figure here this one is not cents-exact.
    pub year_to_date_tax_owed: Decimal,
    pub periods_elapsed: u32,
}

/// The base, rate, and any floor or ceiling applied to one social security
/// figure, for explainability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SscTrace {
    pub basic_pay: Money,
    /// `basic_pay` clamped to `floor`/`ceiling` — the actual base charged.
    pub base: Money,
    pub rate: Decimal,
    pub floor: Money,
    pub ceiling: Money,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PayeResult {
    pub amount: Money,
    pub trace: PayeTrace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SscResult {
    pub amount: Money,
    pub trace: SscTrace,
}

/// The result of calculating one Employment for one PayPeriod.
#[derive(Debug, Clone, PartialEq, Eq)]
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
}

pub fn calculate(
    input: &PayrollInput,
    rules: &PayrollRules,
) -> Result<PayrollCalculation, PayrollError> {
    if !input.employment.has_coherent_dates() {
        return Err(PayrollError::ContradictoryEmploymentDates);
    }

    let terms = input.employment.compensation_terms();
    if !terms.covers(input.period) {
        return Err(PayrollError::CompensationTermsDoNotCoverPeriod);
    }

    let mut earning_lines = vec![Earning::BasicPay(terms.basic_pay())];
    earning_lines.extend(input.earnings.iter().copied());

    let gross_remuneration: Money = earning_lines.iter().map(|line| line.amount()).sum();
    let taxable_remuneration: Money = earning_lines
        .iter()
        .filter(|line| line.is_taxable())
        .map(|line| line.amount())
        .sum();

    let ytd = input.year_to_date;
    let year_to_date_taxable_remuneration = ytd.prior_taxable_remuneration() + taxable_remuneration;
    let year_to_date_tax_owed = rules.tax_owed_on(year_to_date_taxable_remuneration.as_decimal());
    let paye_unrounded = year_to_date_tax_owed - ytd.prior_paye().as_decimal();
    let paye_amount = rules
        .rounding_rule()
        .apply(paye_unrounded)
        .expect("cumulative PAYE never owes less than what was already withheld");

    let social_security = rules.social_security();
    let basic_pay = terms.basic_pay();
    let ssc_base = social_security.base(basic_pay);

    let employee_ssc_amount = rules
        .rounding_rule()
        .apply(ssc_base.as_decimal() * social_security.employee_rate())
        .expect("a non-negative base and rate never produce a negative contribution");
    let employer_ssc_amount = rules
        .rounding_rule()
        .apply(ssc_base.as_decimal() * social_security.employer_rate())
        .expect("a non-negative base and rate never produce a negative contribution");

    let deductions = vec![
        Deduction::Statutory(StatutoryDeduction::PAYE(paye_amount)),
        Deduction::Statutory(StatutoryDeduction::SocialSecurity(employee_ssc_amount)),
    ];

    let net_pay = gross_remuneration
        .checked_sub(paye_amount)
        .and_then(|remainder| remainder.checked_sub(employee_ssc_amount))
        .expect("statutory deductions at these rates never exceed gross remuneration");

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
                periods_elapsed: ytd.periods_elapsed(),
            },
        },
        employee_social_security: SscResult {
            amount: employee_ssc_amount,
            trace: SscTrace {
                basic_pay,
                base: ssc_base,
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
                rate: social_security.employer_rate(),
                floor: social_security.floor(),
                ceiling: social_security.ceiling(),
            },
        },
        deductions,
        net_pay,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::employment::CompensationTerms;
    use crate::pay_period::PayPeriod;
    use crate::rules::{PayeBand, RoundingRule, SocialSecurityRules};
    use chrono::NaiveDate;
    use rust_decimal_macros::dec;

    fn money(amount: Decimal) -> Money {
        Money::from_decimal(amount).unwrap()
    }

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    /// 0-100,000: 0%, 100,000-200,000: 20%, 200,000-400,000: 30%, 400,000+: 40%.
    /// SSC 0.9% each way, floor N$500, ceiling N$11,000 — synthetic test
    /// figures, not a statutory table (that arrives with `ruleset_for` in
    /// a later ticket).
    fn test_rules() -> PayrollRules {
        let bands = vec![
            PayeBand::new(money(dec!(0)), dec!(0.00)),
            PayeBand::new(money(dec!(100000)), dec!(0.20)),
            PayeBand::new(money(dec!(200000)), dec!(0.30)),
            PayeBand::new(money(dec!(400000)), dec!(0.40)),
        ];
        let social_security = SocialSecurityRules::new(
            dec!(0.009),
            dec!(0.009),
            money(dec!(500)),
            money(dec!(11000)),
        );
        PayrollRules::new(bands, social_security, RoundingRule::HalfUpToCents)
    }

    fn test_period() -> PayPeriod {
        PayPeriod::new(date(2026, 1, 26), date(2026, 2, 25)).unwrap()
    }

    fn employment_paying(basic_pay: Decimal) -> EmploymentSnapshot {
        let terms = CompensationTerms::new(date(2025, 1, 1), None, money(basic_pay));
        EmploymentSnapshot::new(date(2025, 1, 1), None, terms)
    }

    fn input_for(basic_pay: Decimal, ytd: YearToDateContext) -> PayrollInput {
        PayrollInput::new(employment_paying(basic_pay), test_period(), Vec::new(), ytd)
    }

    fn ytd(prior_taxable: Decimal, prior_paye: Decimal, periods_elapsed: u32) -> YearToDateContext {
        YearToDateContext::new(money(prior_taxable), money(prior_paye), periods_elapsed)
    }

    /// Asserted across every scenario: gross less all deductions equals net
    /// pay, and employer SSC never appears in the deduction total
    /// (INV-007).
    fn assert_invariants(calc: &PayrollCalculation) {
        let deduction_total: Money = calc.deductions.iter().map(|d| d.amount()).sum();
        assert_eq!(
            calc.gross_remuneration
                .checked_sub(deduction_total)
                .unwrap(),
            calc.net_pay,
            "gross less deductions must equal net pay"
        );
        assert_eq!(
            deduction_total,
            calc.paye.amount + calc.employee_social_security.amount,
            "employer SSC must never appear in the deduction total"
        );
    }

    // PC-001: ordinary monthly salaried employee (baseline).
    #[test]
    fn pc_001_ordinary_monthly_salaried_employee() {
        let input = input_for(dec!(9000.00), ytd(dec!(99000.00), dec!(0.00), 11));
        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(calc.gross_remuneration, money(dec!(9000.00)));
        assert_eq!(calc.taxable_remuneration, money(dec!(9000.00)));
        assert_eq!(calc.paye.amount, money(dec!(1600.00)));
        assert_eq!(calc.employee_social_security.amount, money(dec!(81.00)));
        assert_eq!(calc.employer_social_security.amount, money(dec!(81.00)));
        assert_eq!(calc.net_pay, money(dec!(7319.00)));
        assert_invariants(&calc);
    }

    // PC-002: employee below the PAYE threshold — zero-PAYE path.
    #[test]
    fn pc_002_below_the_paye_threshold() {
        let input = input_for(dec!(5000.00), YearToDateContext::first_period());
        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(calc.paye.amount, Money::ZERO);
        assert_eq!(calc.employee_social_security.amount, money(dec!(45.00)));
        assert_eq!(calc.net_pay, money(dec!(4955.00)));
        assert_invariants(&calc);
    }

    // PC-003: employee crossing a tax bracket — progressive band logic.
    #[test]
    fn pc_003_crossing_a_tax_bracket() {
        let input = input_for(dec!(10000.00), ytd(dec!(95000.00), dec!(0.00), 3));
        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(calc.paye.amount, money(dec!(1000.00)));
        assert_eq!(calc.employee_social_security.amount, money(dec!(90.00)));
        assert_eq!(calc.net_pay, money(dec!(8910.00)));
        assert_invariants(&calc);
    }

    // PC-004: employee crossing multiple brackets — higher-range logic.
    #[test]
    fn pc_004_crossing_multiple_brackets() {
        let input = input_for(dec!(250000.00), ytd(dec!(180000.00), dec!(16000.00), 8));
        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(calc.paye.amount, money(dec!(76000.00)));
        assert_eq!(calc.employee_social_security.amount, money(dec!(99.00)));
        assert_eq!(calc.net_pay, money(dec!(173901.00)));
        assert_invariants(&calc);
    }

    // PC-009: BasicPay above the SSC ceiling — ceiling clamp.
    #[test]
    fn pc_009_above_the_ssc_ceiling() {
        let input = input_for(dec!(20000.00), YearToDateContext::first_period());
        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(calc.employee_social_security.amount, money(dec!(99.00)));
        assert_eq!(
            calc.employee_social_security.trace.base,
            money(dec!(11000.00))
        );
        assert_eq!(calc.net_pay, money(dec!(19901.00)));
        assert_invariants(&calc);
    }

    // PC-010: BasicPay below the SSC floor — floor clamp.
    #[test]
    fn pc_010_below_the_ssc_floor() {
        let input = input_for(dec!(300.00), YearToDateContext::first_period());
        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(calc.employee_social_security.amount, money(dec!(4.50)));
        assert_eq!(
            calc.employee_social_security.trace.base,
            money(dec!(500.00))
        );
        assert_eq!(calc.net_pay, money(dec!(295.50)));
        assert_invariants(&calc);
    }

    // PC-011: mid-year adoption with an OpeningBalance — cumulative PAYE
    // from prior totals entered at adoption, not accumulated by Salt.
    #[test]
    fn pc_011_mid_year_adoption_with_opening_balance() {
        let input = input_for(dec!(50000.00), ytd(dec!(250000.00), dec!(35000.00), 5));
        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(calc.paye.amount, money(dec!(15000.00)));
        assert_eq!(calc.net_pay, money(dec!(34901.00)));
        assert_invariants(&calc);
    }

    // PC-012: second period of a tax year — PAYE net of prior withholding.
    #[test]
    fn pc_012_second_period_of_a_tax_year() {
        let input = input_for(dec!(15000.00), ytd(dec!(110000.00), dec!(2000.00), 1));
        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(calc.paye.amount, money(dec!(3000.00)));
        assert_eq!(calc.net_pay, money(dec!(11901.00)));
        assert_invariants(&calc);
    }

    // PC-015: a corrected earlier period, absorbed forward. The YTD context
    // already reflects the correction — calculate() has no special-cased
    // "correction" path; it is simply given the fixed totals and continues
    // as normal (ADR-0002).
    #[test]
    fn pc_015_corrected_earlier_period_absorbed_forward() {
        let input = input_for(dec!(12000.00), ytd(dec!(108000.00), dec!(1600.00), 3));
        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(calc.paye.amount, money(dec!(2400.00)));
        assert_eq!(calc.net_pay, money(dec!(9501.00)));
        assert_invariants(&calc);
    }

    #[test]
    fn refuses_contradictory_employment_dates() {
        let terms = CompensationTerms::new(date(2025, 1, 1), None, money(dec!(5000.00)));
        let employment = EmploymentSnapshot::new(date(2026, 3, 1), Some(date(2026, 1, 1)), terms);
        let input = PayrollInput::new(
            employment,
            test_period(),
            Vec::new(),
            YearToDateContext::first_period(),
        );

        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::ContradictoryEmploymentDates)
        );
    }

    #[test]
    fn refuses_compensation_terms_that_do_not_cover_the_period() {
        // Effective only from after the period starts.
        let terms = CompensationTerms::new(date(2026, 2, 1), None, money(dec!(5000.00)));
        let employment = EmploymentSnapshot::new(date(2025, 1, 1), None, terms);
        let input = PayrollInput::new(
            employment,
            test_period(),
            Vec::new(),
            YearToDateContext::first_period(),
        );

        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::CompensationTermsDoNotCoverPeriod)
        );
    }
}
