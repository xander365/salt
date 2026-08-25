# Salt

Payroll for Namibian SMEs. Salt calculates, explains, and permanently records what an employer pays an employee for a pay period, under the statutory rules in force at that time.

## Language

### Parties

**Employer**:
The legal entity that employs people and owes remuneration, PAYE, and social security contributions.
_Avoid_: Company, tenant, client, account

**Person**:
A human being, independent of any job they hold.
_Avoid_: Employee (as a record), user

**Employment**:
The relationship between one Person and one Employer over a period of time. The central payroll concept — payroll is calculated per Employment, never per Person.
_Avoid_: Employee, staff member, contract

### Compensation

**CompensationTerms**:
What an Employment agrees to pay, valid over a stated effective period. Never a mutable "current salary" field.
_Avoid_: Salary, package, remuneration terms

**BasicPay**:
The contractual base amount for the period, before allowances, overtime, or any other addition. The base for social security contributions.
_Avoid_: Basic salary, basic wage, base pay

**Earning**:
One classified line of money owed to the employee for the period. Classification, not description, decides tax and contribution treatment.
_Avoid_: Payment, income line, pay item

**RemunerationClassification**:
Deciding which legal category a pay line falls into. It happens before calculation, never inside it — the calculator only ever receives Earnings already classified.
_Avoid_: Tax treatment, categorisation, tagging

**Deduction**:
One classified amount withheld from the employee's remuneration under a stated legal authority.
_Avoid_: Withholding, subtraction, negative earning

**EmployerContribution**:
An amount the Employer owes on top of remuneration. It is an employer cost and never reduces net pay.
_Avoid_: Employer deduction, employer tax

**GrossRemuneration**:
The total of all Earnings for the period, whatever their tax treatment.
_Avoid_: Gross pay, total pay

**TaxableRemuneration**:
The portion of remuneration that PAYE is charged on. Distinct from GrossRemuneration and never derived by summing all pay lines.
_Avoid_: Taxable income, taxable pay

**NetPay**:
GrossRemuneration less all Deductions. What the employee actually receives.
_Avoid_: Take-home, net salary

### Time and rules

**PayPeriod**:
The span of work being paid for, with a start and end date. It does not carry the pay date.
_Avoid_: Pay cycle, month, tax period

**TaxYear**:
The Namibian tax year, 1 March to end of February, against which cumulative PAYE is calculated.
_Avoid_: Fiscal year, financial year

**PayrollRules**:
The set of statutory and agreed calculation rules in force for a PayPeriod, resolved from a PayeTable and an SscRuleset and frozen together. Not itself effective-dated — its two halves are.
_Avoid_: Config, settings, tax tables, ruleset

**PayeTable**:
The progressive annual income-tax bands in force for a stated effective period, named by a PayeTableId. Traceable to a NamRA publication.
_Avoid_: Tax table, brackets, PAYE rules

**SscRuleset**:
The social security contribution rates, minimum and maximum basic wage in force for a stated effective period, named by an SscRulesId. Traceable to a Government Notice.
_Avoid_: SSC config, social security settings

**StatutoryRule**:
A calculation rule traceable to a Tier A source. Distinct from a SaltPolicy, which is what Salt decided where no published rule exists. The two are never described as the same kind of claim.
_Avoid_: Rule (unqualified), compliance rule

**SaltPolicy**:
A calculation choice Salt made because the law is silent or the prescribed method is unpublished. Provisional, stamped with the confirmation it awaits, and never called statutory.
_Avoid_: Convention, our rule, standard practice

**YearToDateContext**:
The TaxYear, plus the taxable remuneration, PAYE withheld, periods elapsed, and PriorEmployment so far in it for one Employment. Required, never optional. Supplied to the calculator; never queried by it. Its first three figures are the OpeningBalance axis and are supported; PriorEmployment is a separate axis and is refused.
_Avoid_: YTD totals, running totals, history

**PeriodsElapsed**:
The Employment's position in the TaxYear, 0 to 11. Not a count of periods the Employment has been paid, and not a count of days worked. Reading it as periods worked over-withholds from every mid-year joiner, which is why the distinction is stated at the type itself.
_Avoid_: Periods worked, months employed

**PriorEmployment**:
Whether the Person had taxable employment with **another Employer** earlier in this TaxYear, and what it paid. Explicitly three-valued: confirmed none, known figures, or unknown. Unknown is refused, never treated as none; known figures are also refused while their treatment is unconfirmed. Never the same thing as an OpeningBalance.
_Avoid_: Previous employer, prior income, opening balance

**OpeningBalance**:
The TaxYear figures **this same Employment** carries into Salt when an Employer adopts Salt mid-year. A starting point for year-to-date, never a running total. Never the same thing as PriorEmployment.
_Avoid_: Opening figures, migration balance, carry-forward

**UnsupportedDeductionStatus**:
What is known about statutory deductions Salt cannot calculate. Three-valued: confirmed none, present with named kinds, or unknown. Present and unknown are both refused. Distinct from a Deduction, which is an amount actually withheld.
_Avoid_: Deduction flag, unsupported deductions (as a bare list)

**ConformanceEvidence**:
The record tying one shipped rule table to its provenance document and the StatutoryCases that prove it. What the verification suite reads instead of source code.
_Avoid_: Compliance record, audit trail

**StatutoryCase**:
One golden example whose expected value is a literal published by a regulator, identified so evidence can reference it. Distinct from a Salt-policy or algorithm case, neither of which can be cited as evidence. The three test-name prefixes — `statutory_*`, `salt_policy_*`, `algorithm_*` — carry the same distinction for a reader.
_Avoid_: Golden test, fixture, test case

### Process

**PayrollInput**:
The complete, self-contained set of facts a calculation needs. If it is not in the PayrollInput, the calculator cannot see it.
_Avoid_: Request, payload, context

**PayrollCalculation**:
The result of calculating one Employment for one PayPeriod. Provisional until finalized.
_Avoid_: Payslip, result, output

**PayrollRun**:
The Employer's act of paying a set of Employments for one PayPeriod. Carries the pay date and moves through Draft, Calculated, Reviewed, Finalized.
_Avoid_: Payroll, batch, cycle

**FinalizedPayroll**:
An immutable record of a PayrollCalculation the Employer has committed to, stored with the exact PayrollInput and PayrollRules that produced it.
_Avoid_: Closed payroll, posted payroll, history

**Reversal**:
An explicit record that cancels one FinalizedPayroll. A correction is a Reversal followed by a replacement calculation; a FinalizedPayroll is never edited.
_Avoid_: Void, undo, rollback, amendment

**Payslip**:
A statutory statement rendered from a FinalizedPayroll. A view, never a source of truth.
_Avoid_: Pay advice, payslip record
