//! The payroll calculator: `calculate(&PayrollInput, &PayrollRules) ->
//! Result<PayrollCalculation, PayrollError>`.
//!
//! A plain function, no trait: it stays a concrete function until a second
//! implementation genuinely exists. Proration, band application, the SSC
//! clamp, and rounding have no exported surface of their own — they are
//! verified only through the values `calculate` returns.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use crate::deduction::{
    Deduction, StatutoryDeduction, VoluntaryDeduction, VoluntaryDeductionInstruction,
};
use crate::earning::{
    DerivedHourlyRate, Earning, EarningInstruction, OvertimeTrace, RemunerationBases,
};
use crate::employment::{CompensationTerms, EmploymentSnapshot, OrdinaryHours};
use crate::money::{Money, MoneyError};
use crate::pay_period::PayPeriod;
use crate::pay_schedule::PaySchedule;
use crate::rules::{BandContribution, PayeTableId, PayrollRules, SscClamp, SscRulesId};
use crate::salt_policy::SaltPolicyStamp;
use crate::tax_year::TaxYear;
use crate::unsupported_deduction::{UnsupportedDeductionKinds, UnsupportedDeductionStatus};
use crate::year_to_date::{
    PeriodsElapsed, PriorEmployment, PriorEmploymentFigures, YearToDateContext,
};

/// The complete, self-contained set of facts one calculation needs. If it
/// is not in the `PayrollInput`, the calculator cannot see it. `schedule`
/// is carried here, not passed beside the input like `PayrollRules`,
/// because the same input must produce the same result every time
/// (INV-002) — a `CompensationTerms.EffectiveFrom` that is valid under one
/// caller-supplied schedule and invalid under another would otherwise make
/// `calculate` non-deterministic in the schedule alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayrollInput {
    employment: EmploymentSnapshot,
    period: PayPeriod,
    /// Earning instructions beyond `BasicPay` — allowances. `calculate`
    /// adds the `BasicPay` line itself from the Employment's
    /// `CompensationTerms`, and the input type cannot express a second one.
    earnings: Vec<EarningInstruction>,
    /// Voluntary deduction instructions (issue #78) — today, only medical
    /// aid premiums. Computed after PAYE and employee social security, from
    /// the earning lines exactly as if no voluntary deduction existed: the
    /// earning-bases accumulator never sees this field. An absent field
    /// decodes as an empty list so a `WorkingPayrollCalculation` written
    /// before issue #78 can still be finalized after upgrading Salt.
    #[serde(default)]
    deductions: Vec<VoluntaryDeductionInstruction>,
    year_to_date: YearToDateContext,
    /// The Employer's `PaySchedule`, used only to validate
    /// `CompensationTerms.EffectiveFrom` against INV-014. It never selects
    /// or generates `period` — the caller supplies that directly.
    schedule: PaySchedule,
    /// What Salt knows about whether the Employee has any of the four
    /// deduction kinds Salt v1 does not support
    /// (`docs/domain/statutory-conformance.md` §3.5, §5.5). There is no
    /// default: a caller must state the fact, and `calculate` refuses
    /// unless it is `ConfirmedNone`.
    unsupported_deductions: UnsupportedDeductionStatus,
}

impl PayrollInput {
    pub fn new(
        employment: EmploymentSnapshot,
        period: PayPeriod,
        earnings: Vec<EarningInstruction>,
        deductions: Vec<VoluntaryDeductionInstruction>,
        year_to_date: YearToDateContext,
        schedule: PaySchedule,
        unsupported_deductions: UnsupportedDeductionStatus,
    ) -> Self {
        PayrollInput {
            employment,
            period,
            earnings,
            deductions,
            year_to_date,
            schedule,
            unsupported_deductions,
        }
    }
}

/// Why a `calculate` call was refused, or a `ruleset_for` lookup could not
/// resolve. Refusals carry the same weight as the arithmetic (INV-012):
/// Salt never guesses at a payroll situation it has no rule for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayrollError {
    /// The Employment's `CompensationTerms` are not in force for every day
    /// of the `PayPeriod` that is actually being paid for. For a
    /// continuing employee that is the whole period; for a joiner or a
    /// leaver it is the days they were employed.
    CompensationTermsDoNotCoverPeriod,
    /// `PayrollInput.period` is not one of the periods the Employer's
    /// `PaySchedule` generates. Without this, INV-014 would be checked
    /// against a schedule the period itself does not follow, and
    /// proration would divide by the length of a period the Employer
    /// never runs (INV-012).
    PayPeriodNotOnTheEmployersSchedule { expected: PayPeriod },
    /// The `PaySchedule` could not name the period around a date, because
    /// that date sits at the extreme edge of the representable calendar.
    /// Unreachable with real dates.
    PayScheduleOutsideRepresentableCalendar { date: NaiveDate },
    /// `CompensationTerms.EffectiveFrom` is not itself a `PayPeriod` start
    /// date for the Employer's `PaySchedule` (INV-014). A pay rise dated
    /// mid-period is refused rather than silently rounded onto the next
    /// period — an Employer must never believe a rise took effect on the
    /// 15th while Salt quietly disagrees.
    ///
    /// Also the refusal `validate_effective_from_is_a_period_start` returns
    /// for any other effective-dated declaration pinned to a `PayPeriod`
    /// start the same way (§4.5c's `UnsupportedDeductionStatus`) — the
    /// requirement is one fact about the schedule, not a fact only
    /// `CompensationTerms` has.
    EffectiveFromNotAPeriodStart {
        next_valid_effective_from: NaiveDate,
    },
    /// The Employment's end date is before its start date.
    ContradictoryEmploymentDates,
    /// The Employment does not overlap the `PayPeriod` at all. A joiner or
    /// leaver — the Employment starting or ending inside the period — is
    /// not this error; it is prorated instead
    /// (`docs/domain/payroll-calculation.md` §8.2).
    EmploymentDoesNotOverlapPeriod,
    /// Recalculating cumulative PAYE against the corrected year-to-date
    /// figures produced a liability lower than what the context says was
    /// already withheld. Refund handling is not modeled anywhere in this
    /// domain yet, so this is refused rather than silently clamped to zero
    /// or turned into a negative deduction (INV-012).
    PriorPayeExceedsRecalculatedLiability,
    /// PAYE, employee social security and any voluntary deduction together
    /// exceeded gross remuneration. Carries the shortfall — how much more
    /// than gross remuneration the deductions came to — so the refusal is
    /// actionable without the caller re-deriving it. Salt invents no
    /// priority order, cap, or carry-forward to resolve this: it refuses and
    /// names the gap (`Q-OPEN-22`).
    DeductionsExceedGrossRemuneration { shortfall: Money },
    /// A monetary amount overflowed `i64` cents during calculation.
    AmountOverflow,
    /// `paye_table_for` found no `PayeTable` whose payroll applicability
    /// interval covers the given `PayPeriod` end date (ADR-0007).
    NoPayeTableCoversDate { date: NaiveDate },
    /// `paye_table_for` found two `PayeTable`s whose payroll applicability
    /// intervals overlap. Detected explicitly rather than resolved by
    /// declaration order or by taking the first match, because a silent
    /// resolution would hide a data bug in the shipped catalogue.
    OverlappingPayeTables {
        first: PayeTableId,
        second: PayeTableId,
    },
    /// `ssc_rules_for` found no `SscRuleset` whose payroll applicability
    /// interval covers the given `PayPeriod` end date (ADR-0007).
    NoSscRulesetCoversDate { date: NaiveDate },
    /// `ssc_rules_for` found two `SscRuleset`s whose payroll applicability
    /// intervals overlap. Detected explicitly rather than resolved by
    /// declaration order or by taking the first match, because a silent
    /// resolution would hide a data bug in the shipped catalogue.
    OverlappingSscRulesets {
        first: SscRulesId,
        second: SscRulesId,
    },
    /// The supplied `PayrollRules`' `PayeTable` payroll applicability
    /// interval does not cover `PayrollInput.period`'s end date
    /// (ADR-0005, ADR-0007). A caller that bypasses `ruleset_for` cannot
    /// use a table that was not in force for the period being calculated.
    /// Names the offending table and the date it fails to cover, so the
    /// refusal is explainable without re-deriving either.
    PayeTableDoesNotCoverPeriod {
        table: PayeTableId,
        period_end: NaiveDate,
    },
    /// The supplied `PayrollRules`' `SscRuleset` payroll applicability
    /// interval does not cover `PayrollInput.period`'s end date
    /// (ADR-0005, ADR-0007). The SSC half of the check above, kept
    /// separate so a caller learns which of the two axes is wrong.
    SscRulesetDoesNotCoverPeriod {
        ruleset: SscRulesId,
        period_end: NaiveDate,
    },
    /// `PayrollInput.year_to_date.tax_year()` is not the `TaxYear`
    /// `TaxYear::for_period_end` resolves for the period's end date
    /// (ADR-0005). A period straddling the tax year end must use the
    /// TaxYear its end date falls in, never the one its start date does.
    WrongTaxYearForPeriod {
        expected: TaxYear,
        supplied: TaxYear,
    },
    /// `PayrollInput.unsupported_deductions` is `Unknown`: nobody has
    /// established whether the Employee has any of the four deduction
    /// kinds Salt v1 does not support. An unasked question must never
    /// pass as a confirmed "no"
    /// (`docs/domain/statutory-conformance.md` §5.5).
    UnsupportedDeductionStatusUnknown,
    /// The Employee has one or more of the four deduction kinds Salt v1
    /// does not support. Every kind supplied is named, not just the
    /// first, so nothing has to be re-established once support ships.
    /// Carries `UnsupportedDeductionKinds`, not a bare `Vec`, so the
    /// refusal itself cannot claim "present" with nothing named.
    UnsupportedDeductionsPresent { kinds: UnsupportedDeductionKinds },
    /// `PayrollInput.year_to_date.prior_employment()` is `Unknown`: nobody
    /// has established whether the Person had taxable employment with
    /// another Employer earlier in this tax year. An unasked question
    /// must never pass as a confirmed `None` (SC-OPEN-4).
    PriorEmploymentUnknown,
    /// An overtime line was supplied, but the `CompensationTerms` row
    /// covering the period records no `OrdinaryHours`. The divisor has no
    /// value without it, and Salt will not invent one such as 173.33 — the
    /// assumption is recorded per Employment precisely so it is visible and
    /// dated (ADR-0022, SC-OPEN-6). Salary-only periods are unaffected: a
    /// legacy row's missing hours are unknown, not invalid.
    OrdinaryHoursNotRecorded,
    /// The Person had taxable employment with another Employer earlier
    /// in this tax year. How a new employer must treat those figures is
    /// unresolved (SC-OPEN-4), so `calculate` refuses rather than guess —
    /// carrying the recorded figures so nothing has to be re-gathered once
    /// the treatment is confirmed.
    PriorEmploymentPresent { figures: PriorEmploymentFigures },
}

impl std::fmt::Display for PayrollError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PayrollError::CompensationTermsDoNotCoverPeriod => {
                write!(f, "compensation terms do not cover every day being paid")
            }
            PayrollError::PayPeriodNotOnTheEmployersSchedule { expected } => {
                write!(
                    f,
                    "the pay period is not one the employer's pay schedule generates; the period around its start date is {} to {}",
                    expected.start(),
                    expected.end()
                )
            }
            PayrollError::PayScheduleOutsideRepresentableCalendar { date } => {
                write!(
                    f,
                    "the pay schedule cannot name the period around {date}: it is outside the representable calendar"
                )
            }
            PayrollError::EffectiveFromNotAPeriodStart {
                next_valid_effective_from,
            } => {
                write!(
                    f,
                    "effective_from must be a pay period start date; the next valid effective date is {next_valid_effective_from}"
                )
            }
            PayrollError::ContradictoryEmploymentDates => {
                write!(f, "employment end date is before its start date")
            }
            PayrollError::EmploymentDoesNotOverlapPeriod => {
                write!(f, "employment does not overlap the pay period")
            }
            PayrollError::PriorPayeExceedsRecalculatedLiability => {
                write!(f, "prior PAYE exceeds recalculated year-to-date liability")
            }
            PayrollError::DeductionsExceedGrossRemuneration { shortfall } => {
                write!(
                    f,
                    "deductions exceed gross remuneration by {}",
                    shortfall.as_decimal()
                )
            }
            PayrollError::AmountOverflow => write!(f, "a monetary amount overflowed"),
            PayrollError::NoPayeTableCoversDate { date } => {
                write!(f, "no PAYE table's payroll applicability covers {date}")
            }
            PayrollError::OverlappingPayeTables { first, second } => {
                write!(
                    f,
                    "PAYE tables {first} and {second} have overlapping payroll applicability"
                )
            }
            PayrollError::NoSscRulesetCoversDate { date } => {
                write!(f, "no SSC ruleset's payroll applicability covers {date}")
            }
            PayrollError::OverlappingSscRulesets { first, second } => {
                write!(
                    f,
                    "SSC rulesets {first} and {second} have overlapping payroll applicability"
                )
            }
            PayrollError::PayeTableDoesNotCoverPeriod { table, period_end } => {
                write!(
                    f,
                    "PAYE table {table}'s payroll applicability does not cover the pay period's end date {period_end}"
                )
            }
            PayrollError::SscRulesetDoesNotCoverPeriod {
                ruleset,
                period_end,
            } => {
                write!(
                    f,
                    "SSC ruleset {ruleset}'s payroll applicability does not cover the pay period's end date {period_end}"
                )
            }
            PayrollError::WrongTaxYearForPeriod { expected, supplied } => {
                write!(
                    f,
                    "the year-to-date context's tax year starting {} does not match the tax year starting {} that the period end date falls in",
                    supplied.starting_year(),
                    expected.starting_year()
                )
            }
            PayrollError::UnsupportedDeductionStatusUnknown => {
                write!(
                    f,
                    "whether the employee has an unsupported deduction has not been established"
                )
            }
            PayrollError::UnsupportedDeductionsPresent { kinds } => {
                write!(
                    f,
                    "the employee has deductions Salt does not calculate: {kinds}"
                )
            }
            PayrollError::OrdinaryHoursNotRecorded => {
                write!(
                    f,
                    "overtime cannot be priced: the compensation terms covering this period record no ordinary hours"
                )
            }
            PayrollError::PriorEmploymentUnknown => {
                write!(
                    f,
                    "whether the employee had taxable employment with another employer earlier this tax year has not been established"
                )
            }
            PayrollError::PriorEmploymentPresent { figures } => {
                write!(
                    f,
                    "the employee had taxable employment with another employer earlier this tax year (taxable remuneration {}, PAYE {}); treatment is unresolved",
                    figures.taxable_remuneration().as_decimal(),
                    figures.paye().as_decimal()
                )
            }
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

/// A condition Salt flags without blocking calculation. Deliberately
/// uninhabited today: `calculate` produces no warnings, so
/// `PayrollCalculation.warnings` is always empty until a variant is added.
/// Anything that must genuinely block is a `PayrollError`, never a
/// `Warning` (`docs/domain/payroll-calculation.md` §6.4).
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
    /// Position in the TaxYear, not periods worked — see
    /// [`PeriodsElapsed`]. Recorded so a reviewer can see which scaling
    /// produced `year_to_date_tax_owed` without re-deriving it.
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
    /// The `PayeTable` and `SscRuleset` that produced this calculation,
    /// each identified independently (ADR-0007) — a durable historical
    /// reference only alongside the resolved values `FinalizedPayroll`
    /// stores (ADR-0004), since either catalogue entry can change in a
    /// later release.
    pub paye_table_id: PayeTableId,
    pub ssc_rules_id: SscRulesId,
    pub paye: PayeResult,
    pub employee_social_security: SscResult,
    /// The Employer's own cost. Never appears in `deductions` and never
    /// reduces `net_pay` (INV-007).
    pub employer_social_security: SscResult,
    /// `PAYE`, employee social security, then every voluntary deduction in
    /// the order instructed — matches
    /// `gross_remuneration - deductions == net_pay`. Ordered as a payslip
    /// prints and net pay is derived: statutory first, voluntary last.
    pub deductions: Vec<Deduction>,
    pub net_pay: Money,
    pub warnings: Vec<Warning>,
}

/// The two halves of the SC-OPEN-6 divisor, named rather than written as
/// literals inside the arithmetic: they are carried into every
/// [`OvertimeTrace`] as data, so the workings show the divisor the figure
/// was actually derived with rather than a sentence repeating it.
const MONTHS_PER_YEAR: Decimal = Decimal::from_parts(12, 0, 0, false, 0);
const WEEKS_PER_YEAR: Decimal = Decimal::from_parts(52, 0, 0, false, 0);

pub fn calculate(
    input: &PayrollInput,
    rules: &PayrollRules,
) -> Result<PayrollCalculation, PayrollError> {
    // The period end date selects the PAYE table, the SSC ruleset, and
    // the TaxYear (ADR-0005, ADR-0007); `ruleset_for` and
    // `TaxYear::for_period_end` are the seams that resolve them, but
    // nothing stops a caller from supplying rules or a YearToDateContext
    // that disagrees with the period being calculated. Checked before
    // anything else so a mismatch can never be masked by a later refusal,
    // and checked as two independent halves so a caller learns which one
    // is wrong.
    if !rules
        .paye_table()
        .payroll_applicability()
        .covers(input.period.end())
    {
        return Err(PayrollError::PayeTableDoesNotCoverPeriod {
            table: rules.paye_table().id().clone(),
            period_end: input.period.end(),
        });
    }
    if !rules
        .ssc_ruleset()
        .payroll_applicability()
        .covers(input.period.end())
    {
        return Err(PayrollError::SscRulesetDoesNotCoverPeriod {
            ruleset: rules.ssc_ruleset().id().clone(),
            period_end: input.period.end(),
        });
    }
    let expected_tax_year = TaxYear::for_period_end(input.period.end());
    if input.year_to_date.tax_year() != expected_tax_year {
        return Err(PayrollError::WrongTaxYearForPeriod {
            expected: expected_tax_year,
            supplied: input.year_to_date.tax_year(),
        });
    }

    // Checked before any arithmetic runs, so a refused input never yields
    // partial figures (`docs/domain/statutory-conformance.md` §5.5): an
    // unsupported deduction is not a number Salt can approximate its way
    // past.
    match &input.unsupported_deductions {
        UnsupportedDeductionStatus::ConfirmedNone => {}
        UnsupportedDeductionStatus::Unknown => {
            return Err(PayrollError::UnsupportedDeductionStatusUnknown);
        }
        UnsupportedDeductionStatus::Present(kinds) => {
            return Err(PayrollError::UnsupportedDeductionsPresent {
                kinds: kinds.clone(),
            });
        }
    }

    // Checked before any arithmetic runs, for the same reason as
    // unsupported deductions above: there is no Salt policy for prior
    // employment with another Employer, only a refusal (SC-OPEN-4).
    match input.year_to_date.prior_employment() {
        PriorEmployment::None => {}
        PriorEmployment::Unknown => {
            return Err(PayrollError::PriorEmploymentUnknown);
        }
        PriorEmployment::Some(figures) => {
            return Err(PayrollError::PriorEmploymentPresent { figures });
        }
    }

    if !input.employment.has_coherent_dates() {
        return Err(PayrollError::ContradictoryEmploymentDates);
    }
    // The period is checked against the schedule before anything is
    // derived from either. Everything below leans on the two agreeing:
    // INV-014 compares `effective_from` against this schedule's period
    // starts, and proration divides by this period's length.
    let scheduled_period = period_containing(input.schedule, input.period.start())?;
    if scheduled_period != input.period {
        return Err(PayrollError::PayPeriodNotOnTheEmployersSchedule {
            expected: scheduled_period,
        });
    }

    let employed = input
        .employment
        .employed_days_within(input.period)
        .ok_or(PayrollError::EmploymentDoesNotOverlapPeriod)?;
    let employed_days = employed.days();
    let period_days = (input.period.end() - input.period.start()).num_days() + 1;

    let terms = input.employment.compensation_terms();
    validate_effective_from_is_a_period_start(input.schedule, terms.effective_from())?;
    if !terms.cover_days(employed.first(), employed.last()) {
        return Err(PayrollError::CompensationTermsDoNotCoverPeriod);
    }
    // Proration (`docs/domain/payroll-calculation.md` §8.2) applies to
    // BasicPay only, and only for a joiner or leaver —
    // `employed_days < period_days`. A continuing employee's
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

    // The salary-to-hourly divisor (D18, ADR-0022): `BasicPay x 12 / 52 /
    // OrdinaryHours`, taken from the **contractual** `BasicPay` and never
    // the prorated one above — a person's hourly rate does not fall because
    // they joined mid-month. The whole formula is Salt's own choice
    // (SC-OPEN-6, `NEEDS CONFIRMATION`); no published Namibian rule
    // prescribing a divisor was found, and nothing may describe it as law.
    //
    // The rate is exact and never rounded. There is exactly one rounding on
    // an overtime line, and it happens at the line below, through the same
    // `RoundingRule` seam every other money figure passes through
    // (SC-OPEN-2).
    // Every instruction the caller supplied is kept as its own line, in the
    // order given, after the derived `BasicPay` line. Lines of the same
    // kind are never merged: a payslip has to be able to show each one, and
    // two overtime lines at the same multiplier round independently rather
    // than being summed and rounded once.
    let mut earning_lines = Vec::with_capacity(input.earnings.len() + 1);
    earning_lines.push(Earning::BasicPay(basic_pay));
    for instruction in &input.earnings {
        let line = match instruction {
            EarningInstruction::TaxableAllowance { amount, label } => Earning::TaxableAllowance {
                amount: *amount,
                label: label.clone(),
            },
            EarningInstruction::Overtime {
                hours,
                multiplier,
                label,
            } => {
                let (ordinary_hours, rate) = derive_hourly_rate(terms)?;
                let (multiplier_numerator, multiplier_denominator) = multiplier.as_ratio();
                let amount_numerator = rate
                    .numerator()
                    .checked_mul(hours.as_hundredths())
                    .and_then(|value| value.checked_mul(multiplier_numerator))
                    .ok_or(PayrollError::AmountOverflow)?;
                let amount_denominator = rate
                    .denominator()
                    .checked_mul(multiplier_denominator)
                    .ok_or(PayrollError::AmountOverflow)?;
                Earning::Overtime {
                    amount: rules
                        .rounding_rule()
                        .apply_cents_fraction(amount_numerator, amount_denominator)?,
                    trace: OvertimeTrace {
                        basic_pay: terms.basic_pay(),
                        ordinary_hours,
                        months_per_year: MONTHS_PER_YEAR,
                        weeks_per_year: WEEKS_PER_YEAR,
                        derived_hourly_rate: rate,
                        hours: *hours,
                        multiplier: *multiplier,
                        policy: SaltPolicyStamp::SALARY_TO_HOURLY_DIVISOR,
                    },
                    label: label.clone(),
                }
            }
        };
        earning_lines.push(line);
    }

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

    // The rate, the floor and the ceiling below are statutory. What Salt
    // does with them here is not: prorating `BasicPay` by calendar days
    // and then clamping it to the *full monthly* floor and ceiling is
    // Salt's own rule (SC-OPEN-3, `NEEDS SSC CONFIRMATION`), and rounding
    // the result half-up is Salt's own rule too (SC-OPEN-2). Both are
    // asserted by `salt_policy_*` tests, and neither may be cited as
    // evidence under ADR-0008.
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

    // Voluntary deductions (issue #78, SC-OPEN-7): computed after PAYE and
    // employee social security, from the instructions alone — no formula, no
    // earning base, no interaction with the arithmetic above. The amount
    // withheld is exactly the amount instructed.
    let mut deductions = vec![
        Deduction::Statutory(StatutoryDeduction::PAYE(paye_amount)),
        Deduction::Statutory(StatutoryDeduction::SocialSecurity(employee_ssc_amount)),
    ];
    for instruction in &input.deductions {
        let VoluntaryDeductionInstruction::MedicalAidPremium(amount) = instruction;
        deductions.push(Deduction::Voluntary(
            VoluntaryDeduction::MedicalAidPremium {
                amount: *amount,
                policy: SaltPolicyStamp::MEDICAL_AID_PREMIUM_UNRELIEVED,
            },
        ));
    }

    // Ordered PAYE, employee social security, then voluntary — the order a
    // payslip prints and net pay is derived in (§0's own words). Every
    // `Money` amount is already non-negative, so the only way this sum or
    // subtraction fails is the total going below zero, i.e. the deductions
    // exceeded gross remuneration; the shortfall is recomputed on that path
    // alone so the ordinary path never pays for it.
    let total_deductions = Money::checked_sum(deductions.iter().copied().map(Deduction::amount))?;
    let net_pay = gross_remuneration
        .checked_sub(total_deductions)
        .map_err(|_| PayrollError::DeductionsExceedGrossRemuneration {
            shortfall: total_deductions
                .checked_sub(gross_remuneration)
                .expect("total_deductions exceeds gross_remuneration on this path by construction"),
        })?;

    Ok(PayrollCalculation {
        earning_lines,
        gross_remuneration,
        taxable_remuneration,
        paye_table_id: rules.paye_table().id().clone(),
        ssc_rules_id: rules.ssc_ruleset().id().clone(),
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

/// The SC-OPEN-6 derivation (D18, ADR-0022): `BasicPay x 12 / 52 /
/// OrdinaryHours`, taken from the **contractual** `BasicPay` and never a
/// prorated one — a person's hourly rate does not fall because they joined
/// mid-month. The whole formula is Salt's own choice, `NEEDS CONFIRMATION`;
/// no published Namibian rule prescribing a divisor was found, and nothing
/// may describe it as law.
///
/// The rate is exact and is never rounded here. There is exactly one
/// rounding on an overtime line and it happens at the line, through the
/// `RoundingRule` seam every other money figure passes through (SC-OPEN-2).
///
/// Returns the `OrdinaryHours` it used alongside the rate, so the trace
/// records the hours the figure was actually derived with rather than
/// reading the terms row a second time and risking a different answer.
fn derive_hourly_rate(
    terms: CompensationTerms,
) -> Result<(OrdinaryHours, DerivedHourlyRate), PayrollError> {
    let ordinary_hours = terms
        .ordinary_hours()
        .ok_or(PayrollError::OrdinaryHoursNotRecorded)?;
    let numerator = i128::from(terms.basic_pay().cents())
        .checked_mul(MONTHS_PER_YEAR.mantissa())
        .ok_or(PayrollError::AmountOverflow)?;
    let denominator = WEEKS_PER_YEAR
        .mantissa()
        .checked_mul(ordinary_hours.as_hundredths())
        .ok_or(PayrollError::AmountOverflow)?;
    let rate =
        DerivedHourlyRate::new(numerator, denominator).ok_or(PayrollError::AmountOverflow)?;
    Ok((ordinary_hours, rate))
}

/// `PaySchedule::period_containing`, with its one calendar-edge failure
/// turned into the refusal `calculate` reports. Named rather than
/// inlined twice so both callers fail the same way.
fn period_containing(schedule: PaySchedule, date: NaiveDate) -> Result<PayPeriod, PayrollError> {
    schedule
        .period_containing(date)
        .ok_or(PayrollError::PayScheduleOutsideRepresentableCalendar { date })
}

/// Validates an effective-dated declaration's `effective_from` against
/// INV-014: it must be the start date of one of `schedule`'s own
/// `PayPeriod`s. `calculate` runs this same check on `CompensationTerms`
/// once a `PayPeriod` is in hand; it is exposed here so a caller recording
/// an effective-dated row — `CompensationTerms`, or `UnsupportedDeductionStatus`
/// (§4.5c) — can make the same refusal before any `PayPeriod` exists to
/// calculate against.
pub fn validate_effective_from_is_a_period_start(
    schedule: PaySchedule,
    effective_from: NaiveDate,
) -> Result<(), PayrollError> {
    let effective_period = period_containing(schedule, effective_from)?;
    if effective_period.start() != effective_from {
        let next_valid_effective_from = effective_period.end().succ_opt().ok_or(
            PayrollError::PayScheduleOutsideRepresentableCalendar {
                date: effective_from,
            },
        )?;
        return Err(PayrollError::EffectiveFromNotAPeriodStart {
            next_valid_effective_from,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::earning::{EarningLabel, OvertimeHours, OvertimeMultiplier};
    use crate::employment::OrdinaryHours;
    use crate::employment::{
        CompensationTerms, EmployerId, EmploymentId, PersonId, PersonReference,
    };
    use crate::pay_schedule::{DayOfMonth, Month, PeriodEndDay};
    use crate::rules::{
        EffectivePeriod, PayeBand, PayeTable, PayeTableId, RoundingRule, SscRulesId, SscRuleset,
    };
    use crate::ruleset::ruleset_for;
    use crate::salt_policy::{SaltPolicyId, SaltPolicyStatus};
    use crate::unsupported_deduction::{UnsupportedDeductionKind, UnsupportedDeductionKinds};
    use crate::year_to_date::PeriodsElapsed;
    use chrono::NaiveDate;
    use rust_decimal_macros::dec;

    fn money(amount: Decimal) -> Money {
        Money::from_decimal(amount).unwrap()
    }

    fn allowance(amount: Decimal) -> EarningInstruction {
        EarningInstruction::TaxableAllowance {
            amount: money(amount),
            label: None,
        }
    }

    fn output_allowance(amount: Decimal) -> Earning {
        Earning::TaxableAllowance {
            amount: money(amount),
            label: None,
        }
    }

    fn overtime(hours: Decimal, multiplier: Decimal) -> EarningInstruction {
        EarningInstruction::Overtime {
            hours: OvertimeHours::new(hours).unwrap(),
            multiplier: OvertimeMultiplier::try_from(multiplier).unwrap(),
            label: None,
        }
    }

    /// `input_for` with overtime instructions and agreed weekly hours on
    /// the compensation terms — the ordinary shape of an overtime period.
    fn overtime_input(
        basic_pay: Decimal,
        ordinary_hours: Decimal,
        instructions: Vec<EarningInstruction>,
    ) -> PayrollInput {
        let terms = CompensationTerms::new(date(2025, 1, 26), None, money(basic_pay))
            .unwrap()
            .with_ordinary_hours(Some(OrdinaryHours::new(ordinary_hours).unwrap()));
        PayrollInput::new(
            snapshot(date(2025, 1, 1), None, terms),
            test_period(),
            instructions,
            Vec::new(),
            ytd(dec!(0), dec!(0), 1),
            test_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        )
    }

    /// The one overtime line a scenario produced, with its workings.
    fn only_overtime_line(calc: &PayrollCalculation) -> (Money, OvertimeTrace) {
        let mut found = calc.earning_lines.iter().filter_map(|line| match line {
            Earning::Overtime { amount, trace, .. } => Some((*amount, *trace)),
            _ => None,
        });
        let line = found.next().expect("an overtime line was expected");
        assert!(found.next().is_none(), "exactly one overtime line expected");
        line
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

    fn test_ssc_rules_id() -> SscRulesId {
        SscRulesId::new("test-ssc-rules")
    }

    /// Thresholds are multiples of 12 so that scaling by
    /// periods_elapsed_inclusive/12 stays exact decimal arithmetic:
    /// 0-120,000: 0%, 120,000-240,000: 20%, 240,000-480,000: 30%,
    /// 480,000+: 40%. SSC 0.9% each way, floor N$500, ceiling N$11,000 —
    /// synthetic test figures, not the statutory tables `ruleset_for`
    /// resolves; most PC-0XX scenarios are indifferent to the actual
    /// rates, so they stay pinned to these round numbers instead.
    fn test_paye_table() -> PayeTable {
        let bands = vec![
            PayeBand::new(money(dec!(0)), dec!(0.00)).unwrap(),
            PayeBand::new(money(dec!(120000)), dec!(0.20)).unwrap(),
            PayeBand::new(money(dec!(240000)), dec!(0.30)).unwrap(),
            PayeBand::new(money(dec!(480000)), dec!(0.40)).unwrap(),
        ];
        PayeTable::new(
            test_paye_table_id(),
            bands,
            date(2000, 1, 1),
            test_applicability(),
        )
        .unwrap()
    }

    /// A single always-zero-rate band, for scenarios that need a valid
    /// `PayeTable` but are indifferent to PAYE itself.
    fn zero_rate_paye_table() -> PayeTable {
        let bands = vec![PayeBand::new(money(dec!(0)), dec!(0.00)).unwrap()];
        PayeTable::new(
            test_paye_table_id(),
            bands,
            date(2000, 1, 1),
            test_applicability(),
        )
        .unwrap()
    }

    fn test_ssc_ruleset() -> SscRuleset {
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

    fn test_rules() -> PayrollRules {
        PayrollRules::new(
            test_paye_table(),
            test_ssc_ruleset(),
            RoundingRule::HalfUpToCents,
        )
    }

    fn test_period() -> PayPeriod {
        PayPeriod::new(date(2026, 1, 26), date(2026, 2, 25)).unwrap()
    }

    /// The schedule `test_period()` (2026-01-26 to 2026-02-25) belongs
    /// to. `calculate` refuses a period its schedule does not generate, so
    /// these two are never mismatched: a test that wants calendar-month
    /// periods uses `calendar_month_schedule()` and a calendar-month
    /// period together.
    fn test_schedule() -> PaySchedule {
        PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap()))
    }

    /// A schedule whose periods are whole calendar months, used by the
    /// proration scenarios so the 28-, 29-, 30-, and 31-day denominators
    /// are easy to read.
    fn calendar_month_schedule() -> PaySchedule {
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
        // 2025-01-26 is a period start under `test_schedule()` (INV-014).
        let terms = CompensationTerms::new(date(2025, 1, 26), None, money(basic_pay)).unwrap();
        snapshot(date(2025, 1, 1), None, terms)
    }

    fn input_for(basic_pay: Decimal, ytd: YearToDateContext) -> PayrollInput {
        input_for_period(basic_pay, test_period(), ytd)
    }

    /// Like `input_for`, but for a period other than `test_period()` — the
    /// ruleset and tax-year selection scenarios need periods that
    /// straddle a specific calendar boundary.
    fn input_for_period(
        basic_pay: Decimal,
        period: PayPeriod,
        ytd: YearToDateContext,
    ) -> PayrollInput {
        PayrollInput::new(
            employment_paying(basic_pay),
            period,
            Vec::new(),
            Vec::new(),
            ytd,
            test_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        )
    }

    fn ytd(prior_taxable: Decimal, prior_paye: Decimal, periods_elapsed: u8) -> YearToDateContext {
        YearToDateContext::new(
            test_tax_year(),
            money(prior_taxable),
            money(prior_paye),
            PeriodsElapsed::new(periods_elapsed).unwrap(),
            PriorEmployment::None,
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
            match line {
                Earning::BasicPay(amount) => {
                    basic_pay_lines += 1;
                    ssc_base = ssc_base.checked_add(*amount).unwrap();
                    taxable = taxable.checked_add(*amount).unwrap();
                    gross = gross.checked_add(*amount).unwrap();
                }
                Earning::TaxableAllowance { amount, .. } => {
                    taxable = taxable.checked_add(*amount).unwrap();
                    gross = gross.checked_add(*amount).unwrap();
                }
                Earning::Overtime { amount, .. } => {
                    taxable = taxable.checked_add(*amount).unwrap();
                    gross = gross.checked_add(*amount).unwrap();
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
            "taxable must be BasicPay plus TaxableAllowance plus Overtime"
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
    fn salt_policy_pc_001_ordinary_monthly_salaried_employee() {
        let input = input_for(dec!(15000.00), ytd(dec!(110000.00), dec!(0.00), 11));
        let calc = calculate(&input, &test_rules()).unwrap();

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
    fn salt_policy_pc_002_below_the_paye_threshold() {
        let input = input_for(
            dec!(5000.00),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
        );
        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(calc.paye.amount, Money::ZERO);
        assert_eq!(calc.employee_social_security.amount, money(dec!(45.00)));
        assert_eq!(calc.net_pay, money(dec!(4955.00)));
        assert_invariants(&calc);
    }

    // PC-003: employee crossing a tax bracket — progressive band logic.
    #[test]
    fn salt_policy_pc_003_crossing_a_tax_bracket() {
        let input = input_for(dec!(15000.00), ytd(dec!(19000.00), dec!(0.00), 2));
        let calc = calculate(&input, &test_rules()).unwrap();

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
    fn salt_policy_pc_004_crossing_multiple_brackets() {
        let input = input_for(dec!(180000.00), ytd(dec!(150000.00), dec!(25000.00), 5));
        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(calc.paye.amount, money(dec!(59000.00)));
        assert_eq!(calc.employee_social_security.amount, money(dec!(99.00)));
        assert_eq!(calc.net_pay, money(dec!(120901.00)));
        assert_invariants(&calc);
    }

    // PC-009: BasicPay above the SSC ceiling — ceiling clamp.
    #[test]
    fn salt_policy_pc_009_above_the_ssc_ceiling() {
        let input = input_for(
            dec!(20000.00),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
        );
        let calc = calculate(&input, &test_rules()).unwrap();

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
    fn salt_policy_pc_010_below_the_ssc_floor() {
        let input = input_for(
            dec!(300.00),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
        );
        let calc = calculate(&input, &test_rules()).unwrap();

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
    fn salt_policy_pc_011_mid_year_adoption_with_opening_balance() {
        let input = input_for(dec!(100000.00), ytd(dec!(200000.00), dec!(32000.00), 7));
        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(calc.paye.amount, money(dec!(26000.00)));
        assert_eq!(calc.net_pay, money(dec!(73901.00)));
        assert_invariants(&calc);
    }

    // PC-012: second period of a tax year — PAYE net of prior withholding.
    #[test]
    fn salt_policy_pc_012_second_period_of_a_tax_year() {
        let input = input_for(dec!(20000.00), ytd(dec!(15000.00), dec!(1000.00), 1));
        let calc = calculate(&input, &test_rules()).unwrap();

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

    // PC-013: period 26 Aug – 25 Sep 2026, straddling the 1 September SSC
    // ceiling change — the period end date selects the ruleset, and the
    // September ruleset's N$12,500 ceiling applies to the entire period.
    // Under the old N$11,000 ceiling this BasicPay would have clamped.
    #[test]
    fn salt_policy_pc_013_period_end_selects_the_ruleset_across_the_ceiling_change() {
        let period = PayPeriod::new(date(2026, 8, 26), date(2026, 9, 25)).unwrap();
        let rules = ruleset_for(period.end()).unwrap();
        let input = input_for_period(
            dec!(12000.00),
            period,
            YearToDateContext::first_period_with_no_prior_employment(TaxYear::for_period_end(
                period.end(),
            )),
        );
        let calc = calculate(&input, &rules).unwrap();

        assert_eq!(
            calc.employee_social_security.trace.base,
            money(dec!(12000.00))
        );
        assert_eq!(calc.employee_social_security.trace.clamp, SscClamp::None);
        assert_eq!(calc.employee_social_security.amount, money(dec!(108.00)));
        assert_eq!(calc.employer_social_security.amount, money(dec!(108.00)));
        assert_eq!(calc.paye.amount, money(dec!(660.00)));
        assert_eq!(calc.net_pay, money(dec!(11232.00)));
        assert_invariants(&calc);
    }

    // The other half of PC-013: the same BasicPay in the period one month
    // earlier, which ends 25 August and so resolves the old ruleset, does
    // clamp at the N$11,000 ceiling. Without this the September figures
    // above would pass even if the ceiling change had never been shipped.
    #[test]
    fn salt_policy_pc_013_the_period_before_the_change_still_clamps_at_the_old_ceiling() {
        let period = PayPeriod::new(date(2026, 7, 26), date(2026, 8, 25)).unwrap();
        let rules = ruleset_for(period.end()).unwrap();
        assert_eq!(rules.ssc_ruleset().id().as_str(), "ssc-2025-03");

        let input = input_for_period(
            dec!(12000.00),
            period,
            YearToDateContext::first_period_with_no_prior_employment(TaxYear::for_period_end(
                period.end(),
            )),
        );
        let calc = calculate(&input, &rules).unwrap();

        assert_eq!(
            calc.employee_social_security.trace.base,
            money(dec!(11000.00))
        );
        assert_eq!(calc.employee_social_security.trace.clamp, SscClamp::Ceiling);
        assert_eq!(calc.employee_social_security.amount, money(dec!(99.00)));
        assert_eq!(calc.employer_social_security.amount, money(dec!(99.00)));
        assert_eq!(calc.paye.amount, money(dec!(660.00)));
        assert_eq!(calc.net_pay, money(dec!(11241.00)));
        assert_invariants(&calc);
    }

    // PC-014: period 26 Feb – 25 Mar 2026, straddling the tax year end —
    // the period end date selects the TaxYear (ADR-0005). The period
    // starts in the tax year starting 2025, but falls wholly in the tax
    // year starting 2026 because that is where its end date lands.
    #[test]
    fn salt_policy_pc_014_period_end_selects_the_tax_year_across_the_year_end() {
        let period = PayPeriod::new(date(2026, 2, 26), date(2026, 3, 25)).unwrap();
        assert_eq!(
            TaxYear::for_period_end(period.start()),
            TaxYear::starting(2025)
        );
        assert_eq!(
            TaxYear::for_period_end(period.end()),
            TaxYear::starting(2026)
        );

        let rules = ruleset_for(period.end()).unwrap();
        let input = input_for_period(
            dec!(8000.00),
            period,
            YearToDateContext::first_period_with_no_prior_employment(TaxYear::for_period_end(
                period.end(),
            )),
        );
        let calc = calculate(&input, &rules).unwrap();

        assert_eq!(calc.paye.amount, Money::ZERO);
        assert_eq!(calc.employee_social_security.amount, money(dec!(72.00)));
        assert_eq!(calc.net_pay, money(dec!(7928.00)));
        assert_invariants(&calc);
    }

    // Rules are passed beside `PayrollInput`, never inside it, precisely so
    // this is possible: the same input, calculated under two rulesets that
    // differ only in their SSC ceiling, gives a different result. Both
    // share `test_applicability()` so this stays a test of that one
    // difference, not of `calculate`'s own applicability check.
    fn rules_with_ceiling(ceiling: Decimal) -> PayrollRules {
        let ssc_ruleset = SscRuleset::new(
            test_ssc_rules_id(),
            dec!(0.009),
            dec!(0.009),
            money(dec!(500)),
            money(ceiling),
            date(2000, 1, 1),
            test_applicability(),
        )
        .unwrap();
        PayrollRules::new(
            zero_rate_paye_table(),
            ssc_ruleset,
            RoundingRule::HalfUpToCents,
        )
    }

    #[test]
    fn salt_policy_varying_only_the_ruleset_against_a_fixed_input_changes_the_result() {
        let input = input_for(
            dec!(12000.00),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
        );

        let before = calculate(&input, &rules_with_ceiling(dec!(11000.00))).unwrap();
        let after = calculate(&input, &rules_with_ceiling(dec!(12500.00))).unwrap();

        assert_eq!(
            before.employee_social_security.trace.clamp,
            SscClamp::Ceiling
        );
        assert_eq!(before.employee_social_security.amount, money(dec!(99.00)));
        assert_eq!(after.employee_social_security.trace.clamp, SscClamp::None);
        assert_eq!(after.employee_social_security.amount, money(dec!(108.00)));
        assert_ne!(
            before.employee_social_security.amount,
            after.employee_social_security.amount
        );
    }

    // ADR-0005: the `PayPeriod` end date selects the SSC ruleset, and a
    // statutory ceiling is a monthly amount that is never split pro-rata.
    // This period runs 26 August to 25 September 2026, straddling the
    // ceiling change, and takes the September ceiling for its whole
    // length — the full N$112.50 per side, never a blend of N$99.00 and
    // N$112.50 weighted by the days each side of 1 September. Asserted
    // through `calculate` rather than through the resolver alone, because
    // "the period is not split" is a claim about the calculated
    // contribution, not only about which ruleset was picked.
    #[test]
    fn salt_policy_a_period_straddling_the_september_2026_ceiling_change_is_not_split() {
        let straddling = PayPeriod::new(date(2026, 8, 26), date(2026, 9, 25)).unwrap();
        let rules = ruleset_for(straddling.end()).unwrap();
        let input = input_for_period(
            dec!(20000.00),
            straddling,
            YearToDateContext::first_period_with_no_prior_employment(TaxYear::for_period_end(
                straddling.end(),
            )),
        );
        let calc = calculate(&input, &rules).unwrap();
        assert_invariants(&calc);

        assert_eq!(calc.ssc_rules_id, SscRulesId::new("ssc-2026-09"));
        assert_eq!(
            calc.employee_social_security.trace.ceiling,
            money(dec!(12500.00))
        );
        assert_eq!(calc.employee_social_security.trace.clamp, SscClamp::Ceiling);
        assert_eq!(calc.employee_social_security.amount, money(dec!(112.50)));
        assert_eq!(calc.employer_social_security.amount, money(dec!(112.50)));

        // The period immediately before it, wholly inside August, takes
        // the old ceiling in full for the same reason.
        let preceding = PayPeriod::new(date(2026, 7, 26), date(2026, 8, 25)).unwrap();
        let preceding_input = input_for_period(
            dec!(20000.00),
            preceding,
            YearToDateContext::first_period_with_no_prior_employment(TaxYear::for_period_end(
                preceding.end(),
            )),
        );
        let preceding_calc =
            calculate(&preceding_input, &ruleset_for(preceding.end()).unwrap()).unwrap();
        assert_invariants(&preceding_calc);

        assert_eq!(preceding_calc.ssc_rules_id, SscRulesId::new("ssc-2025-03"));
        assert_eq!(
            preceding_calc.employee_social_security.amount,
            money(dec!(99.00))
        );
    }

    // The two halves are checked independently, so a caller learns which
    // one is stale (ADR-0007) — PAYE is checked first.
    #[test]
    fn refuses_when_the_paye_table_does_not_cover_the_period_end() {
        let out_of_period_paye_table = PayeTable::new(
            test_paye_table_id(),
            vec![PayeBand::new(Money::ZERO, dec!(0.00)).unwrap()],
            date(2030, 1, 1),
            EffectivePeriod::new(date(2030, 1, 1), None).unwrap(),
        )
        .unwrap();
        let out_of_period_rules = PayrollRules::new(
            out_of_period_paye_table,
            test_ssc_ruleset(),
            RoundingRule::HalfUpToCents,
        );
        let input = input_for(
            dec!(5000.00),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
        );

        assert_eq!(
            calculate(&input, &out_of_period_rules),
            Err(PayrollError::PayeTableDoesNotCoverPeriod {
                table: test_paye_table_id(),
                period_end: test_period().end(),
            })
        );
    }

    #[test]
    fn refuses_when_the_ssc_ruleset_does_not_cover_the_period_end() {
        let out_of_period_ssc_ruleset = SscRuleset::new(
            test_ssc_rules_id(),
            dec!(0.009),
            dec!(0.009),
            money(dec!(500)),
            money(dec!(11000)),
            date(2030, 1, 1),
            EffectivePeriod::new(date(2030, 1, 1), None).unwrap(),
        )
        .unwrap();
        let out_of_period_rules = PayrollRules::new(
            test_paye_table(),
            out_of_period_ssc_ruleset,
            RoundingRule::HalfUpToCents,
        );
        let input = input_for(
            dec!(5000.00),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
        );

        assert_eq!(
            calculate(&input, &out_of_period_rules),
            Err(PayrollError::SscRulesetDoesNotCoverPeriod {
                ruleset: test_ssc_rules_id(),
                period_end: test_period().end(),
            })
        );
    }

    // The 26 Feb - 25 Mar boundary period falls wholly in the tax year
    // starting 2026 (ADR-0005), so a caller supplying the preceding
    // TaxYear — the one its start date falls in — is refused.
    #[test]
    fn refuses_a_tax_year_that_does_not_match_the_periods_end_date() {
        let period = PayPeriod::new(date(2026, 2, 26), date(2026, 3, 25)).unwrap();
        let rules = ruleset_for(period.end()).unwrap();
        let wrong_ytd =
            YearToDateContext::first_period_with_no_prior_employment(TaxYear::starting(2025));
        let input = input_for_period(dec!(8000.00), period, wrong_ytd);

        assert_eq!(
            calculate(&input, &rules),
            Err(PayrollError::WrongTaxYearForPeriod {
                expected: TaxYear::starting(2026),
                supplied: TaxYear::starting(2025),
            })
        );
    }

    // PC-015: a corrected earlier period, absorbed forward. The YTD context
    // already reflects the correction — calculate() has no special-cased
    // "correction" path; it is simply given the fixed totals and continues
    // as normal (ADR-0002).
    #[test]
    fn salt_policy_pc_015_corrected_earlier_period_absorbed_forward() {
        let input = input_for(dec!(25000.00), ytd(dec!(45000.00), dec!(3000.00), 3));
        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(calc.paye.amount, money(dec!(3000.00)));
        assert_eq!(calc.net_pay, money(dec!(21901.00)));
        assert_invariants(&calc);
    }

    fn input_with_unsupported_deductions(status: UnsupportedDeductionStatus) -> PayrollInput {
        PayrollInput::new(
            employment_paying(dec!(25000.00)),
            test_period(),
            Vec::new(),
            Vec::new(),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
            test_schedule(),
            status,
        )
    }

    // §5.5: `ConfirmedNone` is the only state that lets `calculate`
    // proceed — this is the same happy path every other `salt_policy_*`
    // scenario already exercises via `input_for`, asserted directly here.
    #[test]
    fn salt_policy_unsupported_deduction_status_confirmed_none_calculates_normally() {
        let input = input_with_unsupported_deductions(UnsupportedDeductionStatus::ConfirmedNone);
        assert!(calculate(&input, &test_rules()).is_ok());
    }

    // §5.5: an unasked question must never pass as a confirmed "no".
    #[test]
    fn salt_policy_unsupported_deduction_status_unknown_refuses() {
        let input = input_with_unsupported_deductions(UnsupportedDeductionStatus::Unknown);
        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::UnsupportedDeductionStatusUnknown)
        );
    }

    // §5.5: every kind present is named in the refusal, not just the first.
    #[test]
    fn salt_policy_unsupported_deductions_present_refuses_naming_every_kind() {
        let kinds = UnsupportedDeductionKinds::new(vec![
            UnsupportedDeductionKind::ApprovedPensionFund,
            UnsupportedDeductionKind::EducationPolicy,
        ])
        .unwrap();
        let input = input_with_unsupported_deductions(UnsupportedDeductionStatus::Present(kinds));

        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::UnsupportedDeductionsPresent {
                kinds: UnsupportedDeductionKinds::new(vec![
                    UnsupportedDeductionKind::ApprovedPensionFund,
                    UnsupportedDeductionKind::EducationPolicy,
                ])
                .unwrap(),
            })
        );
    }

    // §3.5: all four kinds are refused the same way — no kind is handled
    // more gently than the others.
    #[test]
    fn salt_policy_unsupported_deductions_present_refuses_every_one_of_the_four_kinds() {
        for kind in [
            UnsupportedDeductionKind::ApprovedPensionFund,
            UnsupportedDeductionKind::ProvidentFund,
            UnsupportedDeductionKind::RetirementAnnuityFund,
            UnsupportedDeductionKind::EducationPolicy,
        ] {
            let kinds = UnsupportedDeductionKinds::new(vec![kind]).unwrap();
            let input =
                input_with_unsupported_deductions(UnsupportedDeductionStatus::Present(kinds));

            assert_eq!(
                calculate(&input, &test_rules()),
                Err(PayrollError::UnsupportedDeductionsPresent {
                    kinds: UnsupportedDeductionKinds::new(vec![kind]).unwrap(),
                })
            );
        }
    }

    // §5.5: the refusal runs before any arithmetic, so an input that is
    // also wrong further down still refuses on the deduction — nothing
    // downstream is reached, and no partial figures are produced.
    #[test]
    fn salt_policy_unsupported_deductions_refuse_before_any_arithmetic_runs() {
        let input = PayrollInput::new(
            employment_paying(dec!(25000.00)),
            test_period(),
            Vec::new(),
            Vec::new(),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
            test_schedule(),
            UnsupportedDeductionStatus::Unknown,
        );

        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::UnsupportedDeductionStatusUnknown)
        );
    }

    fn input_with_prior_employment(prior_employment: PriorEmployment) -> PayrollInput {
        PayrollInput::new(
            employment_paying(dec!(25000.00)),
            test_period(),
            Vec::new(),
            Vec::new(),
            YearToDateContext::new(
                test_tax_year(),
                Money::ZERO,
                Money::ZERO,
                PeriodsElapsed::NONE,
                prior_employment,
            ),
            test_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        )
    }

    // §5.6: `None` is the only state that lets `calculate` proceed — the
    // same happy path every other `salt_policy_*` scenario already
    // exercises via `first_period_with_no_prior_employment`, asserted
    // directly here.
    #[test]
    fn salt_policy_prior_employment_none_calculates_normally() {
        let input = input_with_prior_employment(PriorEmployment::None);
        assert!(calculate(&input, &test_rules()).is_ok());
    }

    // §5.6: an unasked question must never pass as a confirmed `None`.
    #[test]
    fn salt_policy_prior_employment_unknown_refuses() {
        let input = input_with_prior_employment(PriorEmployment::Unknown);
        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::PriorEmploymentUnknown)
        );
    }

    // §5.6 / SC-OPEN-4: recorded figures are refused, not consumed — and
    // the figures survive into the error so nothing has to be re-gathered
    // once the treatment is confirmed.
    #[test]
    fn salt_policy_prior_employment_some_refuses_carrying_the_figures() {
        let figures = PriorEmploymentFigures::new(money(dec!(150000.00)), money(dec!(20000.00)));
        let input = input_with_prior_employment(PriorEmployment::Some(figures));

        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::PriorEmploymentPresent {
                figures: PriorEmploymentFigures::new(money(dec!(150000.00)), money(dec!(20000.00))),
            })
        );
    }

    // §5.6 / SC-OPEN-4: the refusal is on the *fact*, not on the size of
    // the figures. Zeroed figures are still prior employment with another
    // Employer, and still refuse.
    #[test]
    fn salt_policy_prior_employment_some_refuses_even_when_the_figures_are_zero() {
        let figures = PriorEmploymentFigures::new(Money::ZERO, Money::ZERO);
        let input = input_with_prior_employment(PriorEmployment::Some(figures));

        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::PriorEmploymentPresent { figures })
        );
    }

    // §5.6: the two facts are never conflated. The same non-zero
    // OpeningBalance (PC-011) still calculates under confirmed `None`, and
    // still refuses under `Some` — and the refusal carries the
    // prior-employment figures, never the OpeningBalance ones.
    #[test]
    fn salt_policy_prior_employment_is_not_conflated_with_an_opening_balance() {
        let opening_balance = ytd(dec!(200000.00), dec!(32000.00), 7);
        assert_eq!(opening_balance.prior_employment(), PriorEmployment::None);

        let adopted = input_for(dec!(100000.00), opening_balance);
        let calc = calculate(&adopted, &test_rules()).unwrap();
        assert_eq!(calc.paye.amount, money(dec!(26000.00)));

        let figures = PriorEmploymentFigures::new(money(dec!(150000.00)), money(dec!(20000.00)));
        let with_prior_employer = input_for(
            dec!(100000.00),
            YearToDateContext::new(
                opening_balance.tax_year(),
                opening_balance.prior_taxable_remuneration(),
                opening_balance.prior_paye(),
                opening_balance.periods_elapsed(),
                PriorEmployment::Some(figures),
            ),
        );

        assert_eq!(
            calculate(&with_prior_employer, &test_rules()),
            Err(PayrollError::PriorEmploymentPresent { figures })
        );
    }

    // §5.6: the refusal runs before any arithmetic, so an input that is
    // also wrong further down still refuses on prior employment — nothing
    // downstream is reached, and no partial figures are produced.
    #[test]
    fn salt_policy_prior_employment_refuses_before_any_arithmetic_runs() {
        let input = PayrollInput::new(
            employment_paying(dec!(25000.00)),
            test_period(),
            Vec::new(),
            Vec::new(),
            YearToDateContext::new(
                test_tax_year(),
                Money::ZERO,
                Money::ZERO,
                PeriodsElapsed::NONE,
                PriorEmployment::Unknown,
            ),
            test_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );

        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::PriorEmploymentUnknown)
        );
    }

    // A first-time Person starting mid tax year (a joiner, PC-005) still
    // calculates correctly when the caller explicitly confirms no prior
    // employment. January is period 11 of the TaxYear starting in March.
    #[test]
    fn salt_policy_first_time_employee_starting_mid_tax_year_calculates_under_confirmed_none() {
        let period = PayPeriod::new(date(2026, 1, 1), date(2026, 1, 31)).unwrap();
        let terms = CompensationTerms::new(date(2026, 1, 1), None, money(dec!(9300.00))).unwrap();
        let employment = snapshot(date(2026, 1, 22), None, terms);
        let ytd = YearToDateContext::new(
            test_tax_year(),
            Money::ZERO,
            Money::ZERO,
            PeriodsElapsed::new(10).unwrap(),
            PriorEmployment::None,
        );
        assert_eq!(ytd.prior_employment(), PriorEmployment::None);
        let input = PayrollInput::new(
            employment,
            period,
            Vec::new(),
            Vec::new(),
            ytd,
            calendar_month_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );

        let calculation = calculate(&input, &test_rules()).unwrap();
        assert_eq!(calculation.gross_remuneration, money(dec!(3000.00)));
        assert_eq!(calculation.paye.amount, Money::ZERO);
        assert_eq!(
            calculation.paye.trace.periods_elapsed,
            PeriodsElapsed::new(10).unwrap()
        );
    }

    #[test]
    fn refuses_contradictory_employment_dates() {
        let terms = CompensationTerms::new(date(2025, 1, 26), None, money(dec!(5000.00))).unwrap();
        let employment = snapshot(date(2026, 3, 1), Some(date(2026, 1, 1)), terms);
        let input = PayrollInput::new(
            employment,
            test_period(),
            Vec::new(),
            Vec::new(),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
            test_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );

        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::ContradictoryEmploymentDates)
        );
    }

    #[test]
    fn refuses_compensation_terms_that_do_not_cover_the_period() {
        // A period start under `test_schedule()`, but the period *after*
        // test_period(): the terms are not in force for any day of it.
        let terms = CompensationTerms::new(date(2026, 2, 26), None, money(dec!(5000.00))).unwrap();
        let employment = snapshot(date(2025, 1, 1), None, terms);
        let input = PayrollInput::new(
            employment,
            test_period(),
            Vec::new(),
            Vec::new(),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
            test_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );

        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::CompensationTermsDoNotCoverPeriod)
        );
    }

    // PC-005: employment starts mid-period — joiner proration. A 31-day
    // calendar-month period (Jan 2026); BasicPay 9,300.00/month is
    // 300.00/day. The employee joins Jan 22, so only Jan 22-31 (10 days)
    // is worked: 300.00 x 10 = 3,000.00, well under the first period's
    // scaled zero-tax band (120,000/12 = 10,000), so PAYE is zero.
    #[test]
    fn salt_policy_pc_005_employment_starts_mid_period_prorates_basic_pay() {
        let period = PayPeriod::new(date(2026, 1, 1), date(2026, 1, 31)).unwrap();
        let terms = CompensationTerms::new(date(2026, 1, 1), None, money(dec!(9300.00))).unwrap();
        let employment = snapshot(date(2026, 1, 22), None, terms);
        let input = PayrollInput::new(
            employment,
            period,
            Vec::new(),
            Vec::new(),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
            calendar_month_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );
        let calc = calculate(&input, &test_rules()).unwrap();

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
    fn salt_policy_pc_006_employment_ends_mid_period_prorates_basic_pay() {
        let period = PayPeriod::new(date(2026, 2, 1), date(2026, 2, 28)).unwrap();
        let terms = CompensationTerms::new(date(2026, 2, 1), None, money(dec!(8400.00))).unwrap();
        let employment = snapshot(date(2025, 1, 1), Some(date(2026, 2, 12)), terms);
        let input = PayrollInput::new(
            employment,
            period,
            Vec::new(),
            Vec::new(),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
            calendar_month_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );
        let calc = calculate(&input, &test_rules()).unwrap();

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

    // A 29-day denominator: February in a leap year. BasicPay 8,700.00
    // over 29 days is 300.00/day; the employee joins Feb 20, so Feb 20-29
    // (10 days) is worked: 300.00 x 10 = 3,000.00. A hardcoded 30-day
    // denominator would give 2,900.00 and a hardcoded 28 would give
    // 3,107.14.
    #[test]
    fn salt_policy_proration_divides_by_a_leap_year_februarys_own_29_days() {
        let period = PayPeriod::new(date(2028, 2, 1), date(2028, 2, 29)).unwrap();
        let terms = CompensationTerms::new(date(2028, 2, 1), None, money(dec!(8700.00))).unwrap();
        let employment = snapshot(date(2028, 2, 20), None, terms);
        let input = PayrollInput::new(
            employment,
            period,
            Vec::new(),
            Vec::new(),
            YearToDateContext::first_period_with_no_prior_employment(TaxYear::for_period_end(
                date(2028, 2, 29),
            )),
            calendar_month_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );
        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(
            calc.earning_lines,
            vec![Earning::BasicPay(money(dec!(3000.00)))]
        );
        assert_eq!(calc.employee_social_security.amount, money(dec!(27.00)));
        assert_eq!(calc.net_pay, money(dec!(2973.00)));
        assert_invariants(&calc);
    }

    // A 30-day denominator: April. BasicPay 9,000.00 over 30 days is
    // 300.00/day; the employee leaves Apr 20, so Apr 1-20 (20 days) is
    // worked: 300.00 x 20 = 6,000.00.
    #[test]
    fn salt_policy_proration_divides_by_a_30_day_periods_own_length() {
        let period = PayPeriod::new(date(2026, 4, 1), date(2026, 4, 30)).unwrap();
        let terms = CompensationTerms::new(date(2026, 4, 1), None, money(dec!(9000.00))).unwrap();
        let employment = snapshot(date(2025, 1, 1), Some(date(2026, 4, 20)), terms);
        let input = PayrollInput::new(
            employment,
            period,
            Vec::new(),
            Vec::new(),
            YearToDateContext::first_period_with_no_prior_employment(TaxYear::for_period_end(
                date(2026, 4, 30),
            )),
            calendar_month_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );
        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(
            calc.earning_lines,
            vec![Earning::BasicPay(money(dec!(6000.00)))]
        );
        assert_eq!(calc.employee_social_security.amount, money(dec!(54.00)));
        assert_eq!(calc.net_pay, money(dec!(5946.00)));
        assert_invariants(&calc);
    }

    // Proration that does not divide evenly is rounded half-up to cents
    // once, on the line (§8.3). 10,000.00 x 10 / 31 = 3,225.80645...,
    // which becomes 3,225.81. SSC is then charged on the rounded line:
    // 3,225.81 x 0.009 = 29.03229 -> 29.03.
    #[test]
    fn salt_policy_a_proration_that_does_not_divide_evenly_is_rounded_half_up_to_cents() {
        let period = PayPeriod::new(date(2026, 1, 1), date(2026, 1, 31)).unwrap();
        let terms = CompensationTerms::new(date(2026, 1, 1), None, money(dec!(10000.00))).unwrap();
        let employment = snapshot(date(2026, 1, 22), None, terms);
        let input = PayrollInput::new(
            employment,
            period,
            Vec::new(),
            Vec::new(),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
            calendar_month_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );
        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(
            calc.earning_lines,
            vec![Earning::BasicPay(money(dec!(3225.81)))]
        );
        assert_eq!(calc.gross_remuneration, money(dec!(3225.81)));
        assert_eq!(calc.employee_social_security.amount, money(dec!(29.03)));
        assert_eq!(calc.net_pay, money(dec!(3196.78)));
        assert_invariants(&calc);
    }

    // A leaver's `CompensationTerms` ordinarily end on their last day.
    // Those terms are in force for every day being paid for, so the
    // calculation prorates exactly as PC-006 does rather than refusing.
    #[test]
    fn salt_policy_a_leaver_whose_terms_end_on_their_last_day_is_prorated_not_refused() {
        let period = PayPeriod::new(date(2026, 2, 1), date(2026, 2, 28)).unwrap();
        let terms = CompensationTerms::new(
            date(2026, 2, 1),
            Some(date(2026, 2, 12)),
            money(dec!(8400.00)),
        )
        .unwrap();
        let employment = snapshot(date(2025, 1, 1), Some(date(2026, 2, 12)), terms);
        let input = PayrollInput::new(
            employment,
            period,
            Vec::new(),
            Vec::new(),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
            calendar_month_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );
        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(
            calc.earning_lines,
            vec![Earning::BasicPay(money(dec!(3600.00)))]
        );
        assert_invariants(&calc);
    }

    // Terms that stop before the employee did leave days unpriced, and
    // Salt has no rule for what those days are worth (INV-012).
    #[test]
    fn refuses_terms_that_end_before_the_last_day_actually_employed() {
        let period = PayPeriod::new(date(2026, 2, 1), date(2026, 2, 28)).unwrap();
        let terms = CompensationTerms::new(
            date(2026, 2, 1),
            Some(date(2026, 2, 10)),
            money(dec!(8400.00)),
        )
        .unwrap();
        let employment = snapshot(date(2025, 1, 1), Some(date(2026, 2, 12)), terms);
        let input = PayrollInput::new(
            employment,
            period,
            Vec::new(),
            Vec::new(),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
            calendar_month_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );

        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::CompensationTermsDoNotCoverPeriod)
        );
    }

    // The schedule validates `effective_from` and the period supplies the
    // proration denominator, so a period the schedule does not generate
    // would make both meaningless. Both a start that is not a period start
    // and an end that does not match are refused, and the refusal names
    // the period the schedule actually runs.
    #[test]
    fn refuses_a_pay_period_that_is_not_one_of_the_schedules_own() {
        let terms = CompensationTerms::new(date(2025, 1, 26), None, money(dec!(5000.00))).unwrap();
        let employment = snapshot(date(2025, 1, 1), None, terms);

        // A 26th-to-25th period offered against a calendar-month schedule.
        let input = PayrollInput::new(
            employment.clone(),
            test_period(),
            Vec::new(),
            Vec::new(),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
            calendar_month_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );
        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::PayPeriodNotOnTheEmployersSchedule {
                expected: PayPeriod::new(date(2026, 1, 1), date(2026, 1, 31)).unwrap(),
            })
        );

        // The right start, the wrong end.
        let truncated = PayPeriod::new(date(2026, 1, 26), date(2026, 2, 24)).unwrap();
        let input = PayrollInput::new(
            employment,
            truncated,
            Vec::new(),
            Vec::new(),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
            test_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );
        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::PayPeriodNotOnTheEmployersSchedule {
                expected: test_period(),
            })
        );
    }

    // Proration applies to BasicPay only: a joiner's allowance is paid in
    // full even though BasicPay is cut down to the days worked.
    #[test]
    fn salt_policy_proration_never_touches_an_allowance() {
        let period = PayPeriod::new(date(2026, 1, 1), date(2026, 1, 31)).unwrap();
        let terms = CompensationTerms::new(date(2026, 1, 1), None, money(dec!(9300.00))).unwrap();
        let employment = snapshot(date(2026, 1, 22), None, terms);
        let input = PayrollInput::new(
            employment,
            period,
            vec![allowance(dec!(800.00))],
            Vec::new(),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
            calendar_month_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );
        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(
            calc.earning_lines,
            vec![
                Earning::BasicPay(money(dec!(3000.00))),
                output_allowance(dec!(800.00)),
            ]
        );
        // The allowance is paid in full: 3,000.00 + 800.00 gross and
        // taxable, and SSC still on the prorated 3,000.00 alone.
        assert_eq!(calc.gross_remuneration, money(dec!(3800.00)));
        assert_eq!(calc.taxable_remuneration, money(dec!(3800.00)));
        assert_eq!(calc.employee_social_security.amount, money(dec!(27.00)));
        assert_invariants(&calc);
    }

    // Twelve consecutive unprorated periods must sum to exactly twelve
    // months' pay. BasicPay deliberately does not divide evenly by any
    // period's day count, so a calculator that always divided by
    // `period_days` — even for a continuing employee — would accumulate
    // rounding drift here; skipping division when the period is fully
    // covered is what keeps the total exact.
    #[test]
    fn salt_policy_twelve_consecutive_full_periods_sum_to_exactly_twelve_months_pay() {
        let schedule = test_schedule();
        let periods = schedule
            .generate_periods(2026, Month::new(1).unwrap(), 12)
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
                Vec::new(),
                YearToDateContext::first_period_with_no_prior_employment(TaxYear::for_period_end(
                    period.end(),
                )),
                schedule,
                UnsupportedDeductionStatus::ConfirmedNone,
            );
            let calc = calculate(&input, &test_rules()).unwrap();
            assert_eq!(
                calc.earning_lines,
                vec![Earning::BasicPay(money(basic_pay))]
            );
            total = total.checked_add(calc.earning_lines[0].amount()).unwrap();
        }

        // 12,345.67 x 12 = 148,148.04, independently calculated.
        assert_eq!(total, money(dec!(148148.04)));
    }

    #[test]
    fn refuses_an_employment_that_does_not_overlap_the_period_at_all() {
        let terms = CompensationTerms::new(date(2025, 1, 26), None, money(dec!(5000.00))).unwrap();
        // The employment ended well before test_period() (2026-01-26 to
        // 2026-02-25) begins — a genuine mismatch, not a leaver.
        let employment = snapshot(date(2025, 1, 1), Some(date(2025, 12, 1)), terms);
        let input = PayrollInput::new(
            employment,
            test_period(),
            Vec::new(),
            Vec::new(),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
            test_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );

        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::EmploymentDoesNotOverlapPeriod)
        );
    }

    #[test]
    fn refuses_an_employment_that_starts_after_the_period() {
        let terms = CompensationTerms::new(date(2025, 1, 26), None, money(dec!(5000.00))).unwrap();
        // The employment starts well after test_period() (2026-01-26 to
        // 2026-02-25) ends — a genuine mismatch, not a joiner.
        let employment = snapshot(date(2026, 3, 1), None, terms);
        let input = PayrollInput::new(
            employment,
            test_period(),
            Vec::new(),
            Vec::new(),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
            test_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );

        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::EmploymentDoesNotOverlapPeriod)
        );
    }

    // Accepts a `CompensationTerms.EffectiveFrom` that is itself a
    // `PayPeriod` start date under the Employer's own 26th-to-25th
    // schedule — the ordinary case INV-014 must not block.
    #[test]
    fn accepts_compensation_terms_effective_on_the_schedules_own_period_start() {
        let schedule = PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap()));
        // test_period() (2026-01-26 to 2026-02-25) itself starts on this
        // schedule's own period-start day.
        let terms = CompensationTerms::new(date(2026, 1, 26), None, money(dec!(5000.00))).unwrap();
        let employment = snapshot(date(2025, 1, 1), None, terms);
        let input = PayrollInput::new(
            employment,
            test_period(),
            Vec::new(),
            Vec::new(),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
            schedule,
            UnsupportedDeductionStatus::ConfirmedNone,
        );

        assert!(calculate(&input, &test_rules()).is_ok());
    }

    // INV-014 across the Feb 28 -> Mar 1 rollover in a non-leap year: a
    // schedule ending periods on the 28th has no Feb 29th to roll onto, so
    // the period after Feb 28 starts Mar 1st, and that — not Feb 29th — is
    // the named next valid date.
    #[test]
    fn refuses_a_pay_rise_across_the_february_28_rollover_in_a_non_leap_year() {
        let schedule = PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(28).unwrap()));
        let period = PayPeriod::new(date(2026, 3, 1), date(2026, 3, 28)).unwrap();
        // 2026-02-28 is the end of the *previous* period, not a start.
        let terms = CompensationTerms::new(date(2026, 2, 28), None, money(dec!(5000.00))).unwrap();
        let employment = snapshot(date(2025, 1, 1), None, terms);
        let input = PayrollInput::new(
            employment,
            period,
            Vec::new(),
            Vec::new(),
            YearToDateContext::first_period_with_no_prior_employment(TaxYear::for_period_end(
                date(2026, 3, 28),
            )),
            schedule,
            UnsupportedDeductionStatus::ConfirmedNone,
        );

        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::EffectiveFromNotAPeriodStart {
                next_valid_effective_from: date(2026, 3, 1),
            })
        );
    }

    // The same rollover in a leap year: Feb 29th exists, so the period
    // after Feb 28th starts Feb 29th, not Mar 1st.
    #[test]
    fn refuses_a_pay_rise_across_the_february_28_rollover_in_a_leap_year() {
        let schedule = PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(28).unwrap()));
        let period = PayPeriod::new(date(2028, 2, 29), date(2028, 3, 28)).unwrap();
        let terms = CompensationTerms::new(date(2028, 2, 28), None, money(dec!(5000.00))).unwrap();
        let employment = snapshot(date(2025, 1, 1), None, terms);
        let input = PayrollInput::new(
            employment,
            period,
            Vec::new(),
            Vec::new(),
            YearToDateContext::first_period_with_no_prior_employment(TaxYear::for_period_end(
                date(2028, 3, 28),
            )),
            schedule,
            UnsupportedDeductionStatus::ConfirmedNone,
        );

        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::EffectiveFromNotAPeriodStart {
                next_valid_effective_from: date(2028, 2, 29),
            })
        );
    }

    // INV-014: a `CompensationTerms.EffectiveFrom` that is not itself a
    // `PayPeriod` start date is refused — this is the "pay rise dated
    // mid-period" case, distinct from a joiner or leaver. The schedule
    // here matches test_period()'s own 26th-to-25th cycle, so the next
    // valid date is 2026-01-26 — test_period()'s own start.
    #[test]
    fn refuses_compensation_terms_not_effective_on_a_period_start() {
        let schedule = PaySchedule::new(PeriodEndDay::Day(DayOfMonth::new(25).unwrap()));
        // 2026-01-10 is before test_period()'s start (so the terms still
        // cover the period) but is not itself a period start.
        let terms = CompensationTerms::new(date(2026, 1, 10), None, money(dec!(5000.00))).unwrap();
        let employment = snapshot(date(2025, 1, 1), None, terms);
        let input = PayrollInput::new(
            employment,
            test_period(),
            Vec::new(),
            Vec::new(),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
            schedule,
            UnsupportedDeductionStatus::ConfirmedNone,
        );

        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::EffectiveFromNotAPeriodStart {
                next_valid_effective_from: date(2026, 1, 26),
            })
        );
    }

    // `validate_effective_from_is_a_period_start` is the same INV-014
    // check `calculate` makes above, exposed directly so a caller recording
    // an effective-dated row can refuse it before any `PayPeriod` exists —
    // see `payroll-app`'s `record_compensation_terms` and
    // `declare_unsupported_deduction_status`.
    #[test]
    fn validate_effective_from_is_a_period_start_accepts_a_period_start() {
        assert_eq!(
            validate_effective_from_is_a_period_start(test_schedule(), date(2026, 1, 26)),
            Ok(())
        );
    }

    #[test]
    fn validate_effective_from_is_a_period_start_refuses_a_mid_period_date() {
        assert_eq!(
            validate_effective_from_is_a_period_start(test_schedule(), date(2026, 1, 10)),
            Err(PayrollError::EffectiveFromNotAPeriodStart {
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
    fn salt_policy_pc_007_taxable_allowance_affects_paye_not_ssc() {
        let input = PayrollInput::new(
            employment_paying(dec!(15000.00)),
            test_period(),
            vec![allowance(dec!(2000.00))],
            Vec::new(),
            ytd(dec!(110000.00), dec!(0.00), 11),
            test_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );
        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(calc.gross_remuneration, money(dec!(17000.00)));
        assert_eq!(calc.taxable_remuneration, money(dec!(17000.00)));
        assert_eq!(calc.paye.amount, money(dec!(1400.00)));
        assert_eq!(calc.employee_social_security.amount, money(dec!(99.00)));
        assert_eq!(calc.net_pay, money(dec!(15501.00)));
        assert_eq!(
            calc.earning_lines,
            vec![
                Earning::BasicPay(money(dec!(15000.00))),
                output_allowance(dec!(2000.00)),
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

    // PC-008: the social security base is a third number, not derivable
    // from gross or from taxable. BasicPay 15,000.00 +
    // TaxableAllowance 5,000.00, period 12 (bands unscaled). The
    // allowance moves gross and taxable by the full 5,000.00 and leaves
    // the social security base exactly where a no-allowance payroll put
    // it, so the three bases are proved apart by comparison with that
    // baseline rather than by literals alone. Year-to-date taxable is
    // 110,000 + 20,000 = 130,000: 10,000 above the 120,000 threshold at
    // 20% is 2,000.00 of PAYE. Net is 20,000.00 - 2,000.00 - 99.00.
    //
    // Since `NonTaxableAllowance` is removed, gross and taxable carry
    // equal amounts here. That is the coincidence of the two v1 kinds,
    // not an identity — they are still accumulated apart, which is what
    // `assert_invariants` re-derives from the returned lines.
    #[test]
    fn salt_policy_pc_008_the_social_security_base_is_independent_of_gross_and_taxable() {
        let ytd_context = ytd(dec!(110000.00), dec!(0.00), 11);
        let input = PayrollInput::new(
            employment_paying(dec!(15000.00)),
            test_period(),
            vec![allowance(dec!(5000.00))],
            Vec::new(),
            ytd_context,
            test_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );
        let calc = calculate(&input, &test_rules()).unwrap();
        let baseline = calculate(&input_for(dec!(15000.00), ytd_context), &test_rules()).unwrap();

        assert_eq!(calc.gross_remuneration, money(dec!(20000.00)));
        assert_eq!(calc.taxable_remuneration, money(dec!(20000.00)));
        assert_eq!(calc.paye.amount, money(dec!(2000.00)));
        assert_eq!(calc.employee_social_security.amount, money(dec!(99.00)));
        assert_eq!(calc.net_pay, money(dec!(17901.00)));

        // Gross and taxable both moved by the whole allowance.
        assert_eq!(
            calc.gross_remuneration
                .checked_sub(baseline.gross_remuneration)
                .unwrap(),
            money(dec!(5000.00))
        );
        assert_eq!(
            calc.taxable_remuneration
                .checked_sub(baseline.taxable_remuneration)
                .unwrap(),
            money(dec!(5000.00))
        );

        // The social security base and both contributions did not.
        assert_eq!(
            calc.employee_social_security.trace.basic_pay,
            baseline.employee_social_security.trace.basic_pay
        );
        assert_eq!(
            calc.employee_social_security.trace.basic_pay,
            money(dec!(15000.00))
        );
        assert_eq!(
            calc.employee_social_security.amount,
            baseline.employee_social_security.amount
        );
        assert_eq!(
            calc.employer_social_security.amount,
            baseline.employer_social_security.amount
        );

        // The social security base is none of the other three totals.
        let ssc_base = calc.employee_social_security.trace.basic_pay;
        assert_ne!(ssc_base, calc.gross_remuneration);
        assert_ne!(ssc_base, calc.taxable_remuneration);
        assert_ne!(ssc_base, calc.net_pay);

        assert_invariants(&calc);
    }

    // A payslip renders the lines it is given, so each one is returned
    // separately and in the order supplied — two allowances of one kind
    // are never collapsed into a single line, even when their amounts are
    // equal.
    #[test]
    fn salt_policy_allowance_lines_are_returned_individually_in_order() {
        let input = PayrollInput::new(
            employment_paying(dec!(15000.00)),
            test_period(),
            vec![
                allowance(dec!(600.00)),
                allowance(dec!(700.00)),
                allowance(dec!(600.00)),
            ],
            Vec::new(),
            ytd(dec!(110000.00), dec!(0.00), 11),
            test_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );
        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(
            calc.earning_lines,
            vec![
                Earning::BasicPay(money(dec!(15000.00))),
                output_allowance(dec!(600.00)),
                output_allowance(dec!(700.00)),
                output_allowance(dec!(600.00)),
            ]
        );
        assert_eq!(calc.gross_remuneration, money(dec!(16900.00)));
        assert_eq!(calc.taxable_remuneration, money(dec!(16900.00)));
        assert_invariants(&calc);
    }

    // A zero-amount allowance is a real line an employer may deliberately
    // record. It is kept and it changes nothing.
    #[test]
    fn salt_policy_a_zero_amount_allowance_is_kept_and_changes_nothing() {
        let with_zero = PayrollInput::new(
            employment_paying(dec!(15000.00)),
            test_period(),
            vec![allowance(dec!(0.00))],
            Vec::new(),
            ytd(dec!(110000.00), dec!(0.00), 11),
            test_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );
        let calc = calculate(&with_zero, &test_rules()).unwrap();
        let baseline = calculate(
            &input_for(dec!(15000.00), ytd(dec!(110000.00), dec!(0.00), 11)),
            &test_rules(),
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
            vec![EarningInstruction::TaxableAllowance {
                amount: Money::from_cents(i64::MAX).unwrap(),
                label: None,
            }],
            Vec::new(),
            ytd(dec!(110000.00), dec!(0.00), 11),
            test_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );

        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::AmountOverflow)
        );
    }

    #[test]
    fn an_allowance_label_changes_no_calculated_amount() {
        let employment = employment_paying(dec!(15000.00));
        let period = test_period();
        let ytd = ytd(dec!(110000.00), dec!(0.00), 11);
        let schedule = test_schedule();
        let rules = test_rules();

        let unlabeled = PayrollInput::new(
            employment.clone(),
            period,
            vec![EarningInstruction::TaxableAllowance {
                amount: money(dec!(2000.00)),
                label: None,
            }],
            Vec::new(),
            ytd,
            schedule,
            UnsupportedDeductionStatus::ConfirmedNone,
        );
        let labeled = PayrollInput::new(
            employment,
            period,
            vec![EarningInstruction::TaxableAllowance {
                amount: money(dec!(2000.00)),
                label: Some(EarningLabel::new("standby allowance").unwrap()),
            }],
            Vec::new(),
            ytd,
            schedule,
            UnsupportedDeductionStatus::ConfirmedNone,
        );

        let unlabeled = calculate(&unlabeled, &rules).unwrap();
        let labeled = calculate(&labeled, &rules).unwrap();

        assert_eq!(
            (
                unlabeled.gross_remuneration,
                unlabeled.taxable_remuneration,
                unlabeled.employee_social_security.trace.basic_pay,
                unlabeled.employee_social_security.trace.base,
                unlabeled.paye.amount,
                unlabeled.employee_social_security.amount,
                unlabeled.employer_social_security.amount,
                unlabeled.net_pay,
            ),
            (
                labeled.gross_remuneration,
                labeled.taxable_remuneration,
                labeled.employee_social_security.trace.basic_pay,
                labeled.employee_social_security.trace.base,
                labeled.paye.amount,
                labeled.employee_social_security.amount,
                labeled.employer_social_security.amount,
                labeled.net_pay,
            )
        );
    }

    #[test]
    fn salt_policy_refuses_when_prior_paye_exceeds_recalculated_liability() {
        // A prior_paye figure inconsistent with any valid history: more tax
        // was supposedly already withheld than the recalculated
        // year-to-date liability can justify.
        let input = input_for(dec!(1000.00), ytd(dec!(1000.00), dec!(999999.00), 0));

        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::PriorPayeExceedsRecalculatedLiability)
        );
    }

    #[test]
    fn refuses_when_deductions_would_exceed_gross_remuneration() {
        // A deliberately pathological 200% employee rate to exercise the
        // refusal path; real rates are validated elsewhere to stay sane.
        let ssc_ruleset = SscRuleset::new(
            test_ssc_rules_id(),
            dec!(2.00),
            dec!(0.009),
            money(dec!(0)),
            money(dec!(11000)),
            date(2000, 1, 1),
            test_applicability(),
        )
        .unwrap();
        let rules = PayrollRules::new(
            zero_rate_paye_table(),
            ssc_ruleset,
            RoundingRule::HalfUpToCents,
        );
        let input = input_for(
            dec!(5000.00),
            YearToDateContext::first_period_with_no_prior_employment(test_tax_year()),
        );

        assert_eq!(
            calculate(&input, &rules),
            Err(PayrollError::DeductionsExceedGrossRemuneration {
                shortfall: money(dec!(5000.00))
            })
        );
    }

    // ---- Overtime (issue #76, ADR-0022) -----------------------------------
    //
    // The divisor these assert is Salt's own choice, SC-OPEN-6, and is
    // `NEEDS CONFIRMATION`. Every test whose expected figure depends on it
    // is therefore named `salt_policy_*` and none may be named `statutory_*`
    // (ADR-0008): a `statutory_*` name means a literal published by a
    // regulator and is citable as evidence, and no regulator published this.
    // Tests of mechanics that hold whatever the divisor is are `algorithm_*`.

    // 12,000 x 12 / 52 / 40 = 69.230769...  N$/hour, exactly and unrounded.
    // Twelve hours at 1.5:  69.230769... x 12 x 1.5 = 1,246.153846... -> 1,246.15.
    // Four hours at 2.0:    69.230769... x 4 x 2.0  =   553.846153... ->   553.85.
    #[test]
    fn salt_policy_overtime_is_hours_at_a_multiplier_priced_by_the_derived_hourly_rate() {
        let input = overtime_input(
            dec!(12000.00),
            dec!(40),
            vec![overtime(dec!(12), dec!(1.5)), overtime(dec!(4), dec!(2.0))],
        );

        let calc = calculate(&input, &test_rules()).unwrap();
        assert_invariants(&calc);

        let amounts: Vec<Money> = calc
            .earning_lines
            .iter()
            .filter_map(|line| match line {
                Earning::Overtime { amount, .. } => Some(*amount),
                _ => None,
            })
            .collect();
        assert_eq!(amounts, vec![money(dec!(1246.15)), money(dec!(553.85))]);
    }

    // Every figure an Operator needs to redo the sum by hand: BasicPay,
    // OrdinaryHours, both halves of the divisor, the unrounded rate, the
    // hours and the multiplier.
    #[test]
    fn salt_policy_overtime_workings_show_every_figure_behind_the_line() {
        let input = overtime_input(
            dec!(12000.00),
            dec!(40),
            vec![overtime(dec!(12), dec!(1.5))],
        );

        let calc = calculate(&input, &test_rules()).unwrap();
        let (amount, trace) = only_overtime_line(&calc);

        assert_eq!(trace.basic_pay, money(dec!(12000.00)));
        assert_eq!(trace.ordinary_hours.as_decimal(), dec!(40));
        assert_eq!(trace.months_per_year, dec!(12));
        assert_eq!(trace.weeks_per_year, dec!(52));
        assert_eq!(trace.hours.as_decimal(), dec!(12));
        assert_eq!(trace.multiplier, OvertimeMultiplier::OneAndAHalf);
        // The repeating rate is held as the exact fraction 900 / 13, never
        // shortened to a finite decimal such as 69.23.
        assert_eq!(trace.derived_hourly_rate.numerator(), 900);
        assert_eq!(trace.derived_hourly_rate.denominator(), 13);
        assert_eq!(amount, money(dec!(1246.15)));
    }

    #[test]
    fn salt_policy_overtime_workings_stamp_the_divisor_as_needing_confirmation() {
        let input = overtime_input(
            dec!(12000.00),
            dec!(40),
            vec![overtime(dec!(12), dec!(1.5))],
        );

        let calc = calculate(&input, &test_rules()).unwrap();
        let (_, trace) = only_overtime_line(&calc);

        assert_eq!(trace.policy.id, SaltPolicyId::SalaryToHourlyDivisor);
        assert_eq!(trace.policy.id.reference(), "SC-OPEN-6");
        assert_eq!(trace.policy.status, SaltPolicyStatus::NeedsConfirmation);
    }

    #[test]
    fn algorithm_overtime_feeds_gross_and_taxable_but_never_the_social_security_base() {
        let input = overtime_input(
            dec!(12000.00),
            dec!(40),
            vec![overtime(dec!(12), dec!(1.5))],
        );

        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(calc.gross_remuneration, money(dec!(13246.15)));
        assert_eq!(calc.taxable_remuneration, money(dec!(13246.15)));
        assert_eq!(
            calc.employee_social_security.trace.basic_pay,
            money(dec!(12000.00)),
            "the social security base must be BasicPay alone"
        );
        assert_eq!(
            calc.employer_social_security.trace.basic_pay,
            money(dec!(12000.00))
        );
    }

    // One rounding, at the line. Two lines at the same multiplier stay two
    // lines and round independently — they are never summed first. At this
    // rate one hour at 1.5 is 103.846153..., which rounds to 103.85 twice
    // (207.70), where summing first would give 207.69.
    #[test]
    fn algorithm_two_overtime_lines_at_the_same_multiplier_round_independently() {
        let input = overtime_input(
            dec!(12000.00),
            dec!(40),
            vec![overtime(dec!(1), dec!(1.5)), overtime(dec!(1), dec!(1.5))],
        );

        let calc = calculate(&input, &test_rules()).unwrap();

        let amounts: Vec<Money> = calc
            .earning_lines
            .iter()
            .filter_map(|line| match line {
                Earning::Overtime { amount, .. } => Some(*amount),
                _ => None,
            })
            .collect();
        assert_eq!(amounts, vec![money(dec!(103.85)), money(dec!(103.85))]);

        let merged = overtime_input(dec!(12000.00), dec!(40), vec![overtime(dec!(2), dec!(1.5))]);
        let merged = calculate(&merged, &test_rules()).unwrap();
        assert_eq!(only_overtime_line(&merged).0, money(dec!(207.69)));
    }

    // A joiner's BasicPay line is prorated; their hourly rate is not. The
    // rate comes from the contractual figure on the CompensationTerms row,
    // because a person's hourly rate does not fall because they joined
    // mid-month. That choice is part of SC-OPEN-6.
    #[test]
    fn salt_policy_overtime_rate_comes_from_contractual_pay_not_the_prorated_figure() {
        let terms = CompensationTerms::new(date(2026, 1, 1), None, money(dec!(12000.00)))
            .unwrap()
            .with_ordinary_hours(Some(OrdinaryHours::new(dec!(40)).unwrap()));
        // Joins on the 16th of a 31-day calendar month: 16 of 31 days.
        let input = PayrollInput::new(
            snapshot(date(2026, 1, 16), None, terms),
            PayPeriod::new(date(2026, 1, 1), date(2026, 1, 31)).unwrap(),
            vec![overtime(dec!(12), dec!(1.5))],
            Vec::new(),
            ytd(dec!(0), dec!(0), 1),
            calendar_month_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );

        let calc = calculate(&input, &test_rules()).unwrap();
        let (amount, trace) = only_overtime_line(&calc);

        assert_eq!(
            calc.earning_lines[0],
            Earning::BasicPay(money(dec!(6193.55))),
            "BasicPay is still prorated"
        );
        assert_eq!(
            trace.basic_pay,
            money(dec!(12000.00)),
            "the rate is derived from contractual pay, not the prorated line"
        );
        assert_eq!(amount, money(dec!(1246.15)));
    }

    #[test]
    fn algorithm_overtime_without_recorded_ordinary_hours_is_refused() {
        let terms = CompensationTerms::new(date(2025, 1, 26), None, money(dec!(12000.00))).unwrap();
        let input = PayrollInput::new(
            snapshot(date(2025, 1, 1), None, terms),
            test_period(),
            vec![overtime(dec!(12), dec!(1.5))],
            Vec::new(),
            ytd(dec!(0), dec!(0), 1),
            test_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );

        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::OrdinaryHoursNotRecorded)
        );
    }

    // A legacy row with no recorded hours still pays salary. Missing hours
    // are unknown, not invalid, and only overtime needs them.
    #[test]
    fn algorithm_a_salary_only_period_does_not_need_recorded_ordinary_hours() {
        let input = input_for(dec!(12000.00), ytd(dec!(0), dec!(0), 1));

        let calc = calculate(&input, &test_rules()).unwrap();

        assert_eq!(calc.gross_remuneration, money(dec!(12000.00)));
    }

    // No new rule for a within-period change: exactly one CompensationTerms
    // row covers every day being paid, and it supplies both the pay and the
    // hours for the whole period. A row dated mid-period is refused by the
    // existing INV-014 message, and nothing about overtime changes that.
    #[test]
    fn salt_policy_overtime_ordinary_hours_change_inside_a_period_is_refused_as_before() {
        let terms = CompensationTerms::new(date(2026, 2, 10), None, money(dec!(12000.00)))
            .unwrap()
            .with_ordinary_hours(Some(OrdinaryHours::new(dec!(45)).unwrap()));
        let input = PayrollInput::new(
            snapshot(date(2025, 1, 1), None, terms),
            test_period(),
            vec![overtime(dec!(12), dec!(1.5))],
            Vec::new(),
            ytd(dec!(0), dec!(0), 1),
            test_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );

        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::EffectiveFromNotAPeriodStart {
                next_valid_effective_from: date(2026, 2, 26),
            })
        );
    }

    #[test]
    fn algorithm_the_overtime_label_is_carried_and_never_changes_the_money() {
        let labelled = PayrollInput::new(
            overtime_input(dec!(12000.00), dec!(40), Vec::new())
                .employment
                .clone(),
            test_period(),
            vec![EarningInstruction::Overtime {
                hours: OvertimeHours::new(dec!(12)).unwrap(),
                multiplier: OvertimeMultiplier::OneAndAHalf,
                label: Some(EarningLabel::new("Sunday overtime").unwrap()),
            }],
            Vec::new(),
            ytd(dec!(0), dec!(0), 1),
            test_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        );
        let unlabelled = overtime_input(
            dec!(12000.00),
            dec!(40),
            vec![overtime(dec!(12), dec!(1.5))],
        );

        let labelled = calculate(&labelled, &test_rules()).unwrap();
        let unlabelled = calculate(&unlabelled, &test_rules()).unwrap();

        assert!(matches!(
            labelled.earning_lines[1],
            Earning::Overtime { ref label, .. } if label.as_ref().unwrap().as_str() == "Sunday overtime"
        ));
        assert_eq!(
            only_overtime_line(&labelled).0,
            only_overtime_line(&unlabelled).0
        );
        assert_eq!(labelled.net_pay, unlabelled.net_pay);
    }

    // ---- Voluntary deductions (issue #78, SC-OPEN-7) ----------------------
    //
    // Salt grants no relief against taxable income for the employee's own
    // medical aid premium: PAYE and social security are computed from the
    // earning lines exactly as if the deduction did not exist, and the
    // premium is subtracted from net pay afterwards. That reading is Salt's
    // own, `NEEDS NAMRA CONFIRMATION`, so every test whose expected figure
    // depends on it is `salt_policy_*`, never `statutory_*` (ADR-0008).

    fn medical_aid(amount: Decimal) -> VoluntaryDeductionInstruction {
        VoluntaryDeductionInstruction::MedicalAidPremium(money(amount))
    }

    /// `input_for` with voluntary deduction instructions alongside no other
    /// earnings beyond `BasicPay`.
    fn input_with_deductions(
        basic_pay: Decimal,
        ytd: YearToDateContext,
        deductions: Vec<VoluntaryDeductionInstruction>,
    ) -> PayrollInput {
        PayrollInput::new(
            employment_paying(basic_pay),
            test_period(),
            Vec::new(),
            deductions,
            ytd,
            test_schedule(),
            UnsupportedDeductionStatus::ConfirmedNone,
        )
    }

    #[test]
    fn salt_policy_a_medical_aid_premium_is_withheld_as_a_voluntary_deduction_and_reduces_net_pay_by_exactly_that_amount()
     {
        let ytd = ytd(dec!(0), dec!(0), 1);
        let without = input_with_deductions(dec!(15000.00), ytd, Vec::new());
        let with = input_with_deductions(dec!(15000.00), ytd, vec![medical_aid(dec!(750.00))]);

        let without = calculate(&without, &test_rules()).unwrap();
        let with = calculate(&with, &test_rules()).unwrap();

        assert_eq!(
            with.net_pay,
            without.net_pay.checked_sub(money(dec!(750.00))).unwrap()
        );
    }

    #[test]
    fn salt_policy_a_medical_aid_premium_changes_neither_paye_nor_either_social_security_figure() {
        let ytd = ytd(dec!(0), dec!(0), 1);
        let without = input_with_deductions(dec!(15000.00), ytd, Vec::new());
        let with = input_with_deductions(dec!(15000.00), ytd, vec![medical_aid(dec!(750.00))]);

        let without = calculate(&without, &test_rules()).unwrap();
        let with = calculate(&with, &test_rules()).unwrap();

        assert_eq!(with.paye, without.paye);
        assert_eq!(
            with.employee_social_security,
            without.employee_social_security
        );
        assert_eq!(
            with.employer_social_security,
            without.employer_social_security
        );
        assert_eq!(with.gross_remuneration, without.gross_remuneration);
        assert_eq!(with.taxable_remuneration, without.taxable_remuneration);
    }

    #[test]
    fn salt_policy_a_medical_aid_premium_is_its_own_classified_line_after_the_statutory_ones() {
        let ytd = ytd(dec!(0), dec!(0), 1);
        let input = input_with_deductions(dec!(15000.00), ytd, vec![medical_aid(dec!(750.00))]);

        let calc = calculate(&input, &test_rules()).unwrap();

        assert!(matches!(
            calc.deductions[0],
            Deduction::Statutory(StatutoryDeduction::PAYE(_))
        ));
        assert!(matches!(
            calc.deductions[1],
            Deduction::Statutory(StatutoryDeduction::SocialSecurity(_))
        ));
        assert_eq!(
            calc.deductions[2],
            Deduction::Voluntary(VoluntaryDeduction::MedicalAidPremium {
                amount: money(dec!(750.00)),
                policy: SaltPolicyStamp::MEDICAL_AID_PREMIUM_UNRELIEVED,
            })
        );
        assert_eq!(calc.deductions.len(), 3);
    }

    #[test]
    fn salt_policy_a_medical_aid_premium_is_stamped_sc_open_7_needs_namra_confirmation() {
        let ytd = ytd(dec!(0), dec!(0), 1);
        let input = input_with_deductions(dec!(15000.00), ytd, vec![medical_aid(dec!(750.00))]);

        let calc = calculate(&input, &test_rules()).unwrap();

        let Deduction::Voluntary(VoluntaryDeduction::MedicalAidPremium { policy, .. }) =
            calc.deductions[2]
        else {
            panic!("expected a voluntary medical aid deduction");
        };
        assert_eq!(policy.id.reference(), "SC-OPEN-7");
        assert_eq!(policy, SaltPolicyStamp::MEDICAL_AID_PREMIUM_UNRELIEVED);
    }

    #[test]
    fn salt_policy_two_medical_aid_lines_are_each_their_own_line_and_both_reduce_net_pay() {
        let ytd = ytd(dec!(0), dec!(0), 1);
        let one_line = input_with_deductions(dec!(15000.00), ytd, vec![medical_aid(dec!(500.00))]);
        let two_lines = input_with_deductions(
            dec!(15000.00),
            ytd,
            vec![medical_aid(dec!(500.00)), medical_aid(dec!(200.00))],
        );

        let one_line = calculate(&one_line, &test_rules()).unwrap();
        let two_lines = calculate(&two_lines, &test_rules()).unwrap();

        assert_eq!(two_lines.deductions.len(), 4);
        assert_eq!(
            two_lines.net_pay,
            one_line.net_pay.checked_sub(money(dec!(200.00))).unwrap()
        );
    }

    #[test]
    fn salt_policy_a_medical_aid_premium_that_would_take_net_pay_below_zero_is_refused_naming_the_shortfall()
     {
        let ytd = ytd(dec!(0), dec!(0), 1);
        // Basic pay 1,000.00 with a low-rate table and SSC leaves a small
        // net pay; a premium larger than what is left must refuse rather
        // than partially withhold.
        let input = input_with_deductions(dec!(1000.00), ytd, vec![medical_aid(dec!(999999.00))]);

        let refusal = calculate(&input, &test_rules()).unwrap_err();

        let PayrollError::DeductionsExceedGrossRemuneration { shortfall } = refusal else {
            panic!("expected DeductionsExceedGrossRemuneration, got {refusal:?}");
        };
        assert!(shortfall > Money::ZERO);
    }

    #[test]
    fn salt_policy_employer_paid_medical_aid_is_refused_by_name_before_any_arithmetic_runs() {
        let kinds =
            UnsupportedDeductionKinds::new(vec![UnsupportedDeductionKind::EmployerPaidMedicalAid])
                .unwrap();
        let input =
            input_with_unsupported_deductions(UnsupportedDeductionStatus::Present(kinds.clone()));

        assert_eq!(
            calculate(&input, &test_rules()),
            Err(PayrollError::UnsupportedDeductionsPresent { kinds })
        );
    }

    #[test]
    fn deserialize_round_trips() {
        let input = input_for(dec!(15000.00), ytd(dec!(110000.00), dec!(0.00), 11));
        let calc = calculate(&input, &test_rules()).unwrap();

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

    #[test]
    fn a_pre_medical_aid_working_input_reads_with_no_deductions() {
        let input = input_for(dec!(15000.00), ytd(dec!(110000.00), dec!(0.00), 11));
        let mut legacy_json = serde_json::to_value(&input).unwrap();
        legacy_json
            .as_object_mut()
            .expect("PayrollInput serializes as an object")
            .remove("deductions");

        assert_eq!(
            serde_json::from_value::<PayrollInput>(legacy_json).unwrap(),
            input
        );
    }
}
