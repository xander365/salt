# Salt

Payroll for Namibian SMEs. Salt calculates, explains, and permanently records what an employer pays an employee for a pay period, under the statutory rules in force at that time.

## Language

### Parties

**Employer**:
The legal entity that employs people and owes remuneration, PAYE, and social security contributions.
_Avoid_: Company, tenant, client, account

**EmployerParticulars**:
The Employer's registered name, addresses and statutory registration numbers as they must appear on a document Salt prints. Master data, correctable with a stated reason — and frozen into every FinalizedPayroll, so correcting them can never rewrite a payslip already issued.
_Avoid_: Company details, employer info, profile

**Person**:
A human being, independent of any job they hold. What Salt *records* is narrower: a Person is known to one Employer, so the same human employed by two Employers is two records (ADR-0020). Salt cannot substantiate that two records are one human, and saying so would tell one Employer where its people work elsewhere.
_Avoid_: Employee (as a record), user

**PersonParticulars**:
The Person's full name, identity number and address as they must appear on a document Salt prints. Correctable with a stated reason, like EmployerParticulars, and frozen into every FinalizedPayroll for the same reason.
_Avoid_: Employee details, personal info, demographics

**Employment**:
The relationship between one Person and one Employer over a period of time. The central payroll concept — payroll is calculated per Employment, never per Person.
_Avoid_: Employee, staff member, contract

**Operator**:
A human being who signs into Salt and acts on an Employer's payroll. Distinct from a Person, who is the subject of an Employment and never signs in. An Operator is one global identity that gains access to Employers through EmployerMembership, so the same human working for two Employers is one Operator, not two.
_Avoid_: User, account, login

**EmployerMembership**:
The grant that lets one Operator act on one Employer, carrying a role. It is the whole of authorization: an EmployerId a caller happens to know grants nothing. Two roles exist — **Owner**, who may also administer memberships and Employer configuration, and **PayrollOperator**, who may run payroll and nothing else.
_Avoid_: Permission, access, tenancy, role (unqualified)

### Compensation

**CompensationTerms**:
What an Employment agrees to pay, valid over a stated effective period. Never a mutable "current salary" field. Master data, not history — correctable with a stated reason even after payroll has been paid against it, because a FinalizedPayroll explains itself and never looks back at these. Carries the BasicPay and the OrdinaryHours the DerivedHourlyRate is built from.
_Avoid_: Salary, package, remuneration terms

**BasicPay**:
The contractual base amount for the period, before allowances, overtime, or any other addition. The base for social security contributions.
_Avoid_: Basic salary, basic wage, base pay

**OrdinaryHours**:
The hours per week an Employment's BasicPay is agreed to cover, held on CompensationTerms and effective-dated with it. It exists to make the DerivedHourlyRate's assumption visible and per-person, rather than hidden inside a constant. Not hours worked, and not hours payable.
_Avoid_: Working hours, standard hours, hours per week (unqualified)

**DerivedHourlyRate**:
The hourly rate Salt computes for a salaried Employment in order to price Overtime: BasicPay x 12 / 52 / OrdinaryHours. A SaltPolicy, never a StatutoryRule — no published rule prescribing the divisor was found. Distinct from a contracted hourly rate, which an hourly-paid Employment would agree and Salt does not yet support.
_Avoid_: Hourly rate (unqualified), rate of pay, divisor

**Earning**:
One classified line of money owed to the employee for the period. Classification, not description, decides tax and contribution treatment.
_Avoid_: Payment, income line, pay item

**Overtime**:
An Earning for hours worked beyond ordinary hours, priced at a DerivedHourlyRate times an OvertimeMultiplier. It feeds GrossRemuneration and TaxableRemuneration and never the social security base, because the Social Security General Regulations exclude overtime from `basic wage`. Salt receives the hours; the attendance system owns the rules that decided which hours they are.
_Avoid_: OT, extra hours, additional pay

**OvertimeMultiplier**:
The factor an Overtime line is priced at — 1.5 or 2.0, a closed set. It is the classification, not a reason: Salt knows the factor and never why the factor applies. A free-text label may carry the human reason and never affects money.
_Avoid_: Overtime rate, OT type, premium

**RemunerationClassification**:
Deciding which legal category a pay line falls into. It happens before calculation, never inside it — the calculator only ever receives Earnings already classified.
_Avoid_: Tax treatment, categorisation, tagging

**Deduction**:
One classified amount withheld from the employee's remuneration under a stated authority — a statute, or the employee's own standing instruction. Classification, not description, decides whether it touches PAYE or the social security base.
_Avoid_: Withholding, subtraction, negative earning

**StatutoryDeduction**:
A Deduction the law requires — PAYE and social security. Salt calculates the amount from PayrollRules; nobody types it.
_Avoid_: Tax, compulsory deduction

**VoluntaryDeduction**:
A Deduction withheld on the employee's own standing instruction, a medical aid premium being the ordinary case. Withheld after tax: it reduces NetPay and never TaxableRemuneration or the social security base. Distinct from an UnsupportedDeductionKind, which Salt refuses precisely because it *would* change PAYE.
_Avoid_: Other deduction, private deduction, after-tax deduction (as a category)

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

**StandingPayItem**:
An Earning or Deduction an Employment carries with effective dates, which every PayrollRun proposes on its own. It is what stops an operator retyping the same allowance twelve times a year. A pay line typed directly onto a run is not one, and never recurs.
_Avoid_: Recurring item, fixed deduction, template line

**RunOverride**:
A change to, or removal of, one StandingPayItem for one PayrollRun only. It records what the run actually paid without touching the standing record, which is what keeps a one-month variation from rewriting master data.
_Avoid_: Adjustment, one-off change, exception

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
The TaxYear figures **this same Employment** carries into Salt when an Employer adopts Salt mid-year, together with the SaltCoverageStart they run up to. An affirmative statement someone makes, never a default Salt writes. A starting point for year-to-date, never a running total. Never the same thing as PriorEmployment.
_Avoid_: Opening figures, migration balance, carry-forward

**SaltCoverageStart**:
The first PayPeriod Salt is responsible for, for one Employment in one TaxYear. Every earlier period of that TaxYear is pre-Salt and accounted for inside the OpeningBalance. Recorded once and frozen, which is what separates a legitimate mid-year adoption from a period an Employer simply forgot to process.
_Avoid_: Go-live date, adoption date, migration cutoff

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

**WorkingCalculation**:
The single latest PayrollCalculation held for one Employment in one PayrollRun before finalization. Replaceable by design — recalculating overwrites it, and it is never history.
_Avoid_: Draft payslip, provisional calculation, pending payroll

**PayrollRun**:
The Employer's act of paying a set of Employments for one PayPeriod. Carries the pay date and moves through Draft, Calculated, Finalized. There is no Reviewed state: finalizing is itself the deliberate approval.
_Avoid_: Payroll, batch, cycle

**CorrectionRun**:
A PayrollRun that puts right one Employment's payroll for a PayPeriod already finalized. Distinct from the one Ordinary run a PayPeriod may have, and the reason an Employer may have more than one run for the same PayPeriod. Holds exactly one Employment, chosen deliberately — an Ordinary run proposes everyone and makes exclusion the deliberate act; a CorrectionRun proposes nobody and makes inclusion the deliberate act. Always carries a stated reason.
_Avoid_: Adjustment run, re-run, supplementary payroll

**FinalizedPayroll**:
An immutable record of a PayrollCalculation the Employer has committed to, stored with the exact PayrollInput, PayrollRules, EmployerParticulars, PersonParticulars and PayslipTemplateVersion that produced it. The **sole** explanation of that payroll — nothing about it is ever rebuilt from what the master records say today.
_Avoid_: Closed payroll, posted payroll, history

**Reversal**:
An explicit record that cancels one FinalizedPayroll. A correction is a Reversal followed by a replacement calculation; a FinalizedPayroll is never edited.
_Avoid_: Void, undo, rollback, amendment

**Live**:
Said of the one FinalizedPayroll that currently counts for an Employment and PayPeriod. A Reversal takes a record out of the live set; a Replacement puts one back. Only live records feed YearToDateContext.
_Avoid_: Active, current, valid, unreversed

**Replacement**:
The FinalizedPayroll that takes a reversed one's place for the same Employment and PayPeriod. Optional — a Reversal on its own is complete. It names the record it replaces, and each reversed record is replaced at most once, so repeated corrections read as a chain. A CorrectionRun that pays someone wrongly left out of a finalized run has nothing to name, and states its reason instead.
_Avoid_: Correction, redo, amended payroll

**Resolved**:
Said of a PayPeriod for one Employment when Salt holds an affirmative account of it: the Employment did not yet exist, the period is before the SaltCoverageStart, a live FinalizedPayroll exists, or someone recorded with a reason that no payroll was owed. Absence of a record never resolves a period — that is the gap Salt refuses to let disappear quietly.
_Avoid_: Closed, complete, done, processed

**SaltVersion**:
The identifier of the Salt release that produced a FinalizedPayroll, recorded so a future maintainer can find the exact code behind a historical figure.
_Avoid_: Build number, app version, release tag

**ActionLog**:
The append-only record of who did what and when. Separate from payroll history, which is the immutable records themselves.
_Avoid_: Audit trail, event log, history

**Payslip**:
A statutory statement rendered from a FinalizedPayroll. A view, never a source of truth. Rendered on demand and never stored: asking for it again renders it again. That is only safe because everything it prints — figures, rules, particulars and template version — froze with the FinalizedPayroll.
_Avoid_: Pay advice, payslip record

**PayslipTemplateVersion**:
The identifier of the layout that a Payslip is rendered with, frozen on the FinalizedPayroll beside the SaltVersion. It is what lets an unstored document be re-rendered years later without silently changing. Its consequence is that a retired template's renderer is never deleted.
_Avoid_: Template, layout version, format

**PayrollRegister**:
Every Employment in one PayrollRun with its figures and the run's totals. A view over FinalizedPayrolls, never a source of truth.
_Avoid_: Payroll report, summary, run listing

**PaymentSummary**:
Who is to be paid what, for one PayrollRun. A view, and an instruction to a human — producing one never means money moved, and it is deliberately not a bank import file.
_Avoid_: Payment file, bank file, EFT export
