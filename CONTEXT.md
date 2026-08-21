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
The set of statutory and agreed calculation rules in force for a stated effective period. Identified by a RulesetId so a historical result can name the rules that produced it.
_Avoid_: Config, settings, tax tables

**YearToDateContext**:
The taxable remuneration, PAYE withheld, and periods elapsed so far in the TaxYear for one Employment. Supplied to the calculator; never queried by it.
_Avoid_: YTD totals, running totals, history

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
