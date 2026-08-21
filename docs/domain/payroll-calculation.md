# Salt — Payroll Calculation Domain Design

**Status:** Grilled. Decisions settled, ready for `/to-spec`.
**Product:** Salt
**Initial market:** Namibian SMEs
**Date:** 2026-08-21

---

## 1. Purpose of this document

This document records the settled domain design for Salt's payroll calculation core. It came out of a `/grill-with-docs` session over the pre-specification draft; the open questions in that draft have been answered and are recorded here as decisions.

Companion documents:

- `CONTEXT.md` — the glossary. Every term below is defined there.
- `docs/adr/0001`–`0006` — the six decisions that were hard to reverse, with their rejected alternatives.

This document is still **not** a database schema, an API specification, a UI specification, or a Rust module layout. It is the agreed meaning of payroll in Salt.

The first design goal:

> Salt must be able to calculate, explain, reproduce, review, and finalize the payroll of an ordinary Namibian SME employee without coupling payroll logic to HTTP, React, PostgreSQL, or a desktop shell.

The larger engineering goal:

> Humans own payroll meaning, architecture, invariants, statutory interpretation, and acceptance criteria. AI agents may perform most tactical implementation work inside those agreed boundaries.

---

## 2. Product context

Salt is a clean-sheet payroll product for Namibian SMEs.

Technical direction:

- Rust backend;
- React + TypeScript frontend;
- PostgreSQL persistence;
- Axum HTTP API;
- SQLx-style explicit SQL rather than a large ORM abstraction;
- vertical slices for application features;
- deep domain modules with small public seams;
- Tauri as a possible desktop shell later.

The architecture preserves the spirit of a FastEndpoints + Dapper + PostgreSQL vertical-slice application: explicit request/use-case boundaries, explicit data access, strong domain rules, little framework magic, features grouped by capability, tests at meaningful seams.

Rust is an implementation material and a source of compiler-enforced constraints, not the point of the exercise.

---

## 3. Design principles

### 3.1 Domain before infrastructure

Begin from payroll concepts and business operations, not controllers, routes, tables, repositories, DTO folders, or screens. Infrastructure serves the payroll model.

### 3.2 The calculator is a pure function

```text
PayrollInput + PayrollRules  ->  PayrollCalculation
```

The calculator does not query PostgreSQL, call services, read the clock, read environment variables, render documents, or mutate anything. For the same input and rules it produces the same result.

### 3.3 Historical payroll is explainable

Salt answers "why did this employee receive this result for this historical period?" from stored history alone — never by re-reading today's mutable employee record.

### 3.4 Statutory rules are effective-dated

Rules apply to a defined period and are identified by a `RulesetId`. Rule changes are ordinary, not invasive. Two real examples already in scope: the SSC ceiling moved N$9,000 → N$11,000 on 1 March 2025, and N$11,000 → N$12,500 on 1 September 2026.

### 3.5 Finalized payroll is history

```text
Draft -> Calculated -> Reviewed -> Finalized
```

A finalized result is immutable. Corrections are explicit (§10).

### 3.6 Explainability is part of correctness

`net_pay = N$17,463.82` alone is not an answer. Salt exposes the components that produced it, as structured data.

### 3.7 Money is exact decimal

No `f32`, no `f64`, ever. Rounding is a domain rule and lives in `PayrollRules`.

### 3.8 No premature internationalisation

Salt v1 is Namibia. Use `PayrollRules`, not a generic multi-country rules framework. Refactor from proven need if another jurisdiction becomes real.

---

## 4. Core domain vocabulary

Definitions live in `CONTEXT.md`. This section records the shapes and the decisions attached to them.

### 4.1 Employer

The legal employing entity. Every payroll record carries an `EmployerId` from day one, so supporting many employers per workspace later is additive rather than a rewrite. **v1 exposes one Employer per workspace in the UI.**

```text
Employer
- EmployerId
- LegalName
- TradingName?
- TaxRegistrationNumber?
- SocialSecurityRegistrationNumber?
- PaySchedule
```

### 4.2 Person

The human being, independent of any job. Only payroll-relevant personal information enters the payroll domain.

```text
Person
- PersonId
- LegalName
- IdentityInformation
- ContactInformation
```

### 4.3 Employment

The relationship between a Person and an Employer. **Employment, not Employee, is the central payroll concept** — payroll is always calculated per Employment.

```text
Employment
- EmploymentId
- PersonId
- EmployerId
- StartDate
- EndDate?
- Occupation
- Status
- CompensationTerms (effective-dated series)
```

**Decision:** the model permits a Person to hold more than one Employment, but **v1 refuses to calculate concurrent live Employments** and says so explicitly (INV-012). This keeps the shape correct without taking on multi-employer tax interaction now.

### 4.4 CompensationTerms

What the Employment agrees to pay, over an effective period. Never a mutable `employee.salary` field.

```text
CompensationTerms
- EffectiveFrom
- EffectiveUntil?
- BasicPay (monthly amount)
- RecurringAllowances
```

**Decision:** `EffectiveFrom` **must be a PayPeriod start date** for that Employer. Mid-period effective dates are rejected with a message naming the next valid date. Pay changes therefore always take effect at a period boundary, and proration only ever concerns joiners and leavers. Silent rounding to the next period is not acceptable — an employer must not believe a rise started on the 15th while Salt quietly disagrees.

**v1 supports monthly BasicPay only.** Hourly, daily, and weekly bases are refused.

### 4.5 PayPeriod and PaySchedule

See ADR-0005. Most Namibian SMEs run a 26th-to-25th cycle, so a PayPeriod is **not** assumed to be a calendar month.

```text
PaySchedule (on Employer)
- Frequency: Monthly          (v1: monthly only)
- PeriodEndDay: 1..28 | LastDayOfMonth

PayPeriod
- StartDate
- EndDate
```

Rules keyed on the **period end date**:

- it selects the applicable `PayrollRules`;
- it selects the `TaxYear`.

A 26 Aug – 25 Sep period uses September's ruleset for the whole period; statutory ceilings are monthly amounts and are never split pro-rata. A 26 Feb – 25 Mar period falls entirely in the new tax year, giving every Employer exactly 12 periods per tax year.

**The pay date belongs to the PayrollRun, not the PayPeriod.** The period is the work being paid for; the pay date is when the Employer actually paid, and two runs for one period can pay on different days.

### 4.6 PayrollRules

The statutory and agreed calculation rules in force for an effective period, as typed Rust (ADR-0003).

```text
PayrollRules
- RulesetId
- EffectivePeriod
- PAYEBands
- SocialSecurityRules (rate, floor, ceiling)
- RoundingRule
```

Rules are never scattered as magic constants across unrelated source files.

---

## 5. Payroll input

The calculator receives a complete input and never reaches into persistence.

```text
PayrollInput
- EmploymentSnapshot
- PayPeriod
- Earnings
- YearToDateContext
- PaySchedule
```

`PaySchedule` is carried inside `PayrollInput`, not passed beside it: it exists only to validate `CompensationTerms.EffectiveFrom` against INV-014 (it never selects or generates `PayPeriod` — the caller supplies that directly), but the same `PayrollInput` must produce the same result every time (INV-002). Passing it beside the input, the way `PayrollRules` is, would let one caller-supplied schedule accept a `CompensationTerms` that another schedule rejects for the identical `PayrollInput` — a hidden second axis of determinism that finalization (§10) does not freeze.

`PayrollRules` is passed **beside** the input:

```rust
fn calculate(
    input: &PayrollInput,
    rules: &PayrollRules,
) -> Result<PayrollCalculation, PayrollError>
```

This makes the caller choose a ruleset deliberately, and it reads clearly in tests: same input, different ruleset, different answer.

### 5.1 EmploymentSnapshot

The payroll-relevant employment facts captured for this calculation, so a historical result is never reinterpreted through today's employment record.

```text
EmploymentSnapshot
- EmploymentId
- EmployerId
- PersonReference
- EmploymentStartDate
- EmploymentEndDate?
- CompensationTerms applicable to this period
```

### 5.2 Earnings

**v1 supports exactly three Earning kinds:**

```text
Earning
- BasicPay
- TaxableAllowance
- NonTaxableAllowance     (travel / subsistence)
```

This is the smallest set that exercises every distinction Salt claims to make:

| | SSC base | PAYE base | Gross |
|---|---|---|---|
| BasicPay | yes | yes | yes |
| TaxableAllowance | no | yes | yes |
| NonTaxableAllowance | no | no | yes |

Overtime, night work, Sunday work, public holiday work, commission, and bonus are refused in v1 and added later. A single `taxable: bool` flag is explicitly rejected as too weak.

### 5.3 Benefits

Out of scope for v1. No benefit-in-kind is modelled.

### 5.4 Deductions

**v1 calculates PAYE and employee SSC only.** Any other requested deduction is refused with "not supported yet".

This removes the Labour Act one-third aggregate cap, deduction ordering, insufficient-net-pay handling, and written-authorization evidence from v1 entirely. The `Deduction` type is still classified from day one, so adding voluntary deductions later is additive:

```text
Deduction
- Statutory(PAYE | SocialSecurity)
- ... further classifications added when supported
```

Salt never accepts `description + amount` and deducts it without a classification and an authority (INV-008).

### 5.5 YearToDateContext

Cumulative PAYE (ADR-0001) makes this mandatory, not optional.

```text
YearToDateContext
- TaxYear
- PriorTaxableRemuneration
- PriorPAYE
- PeriodsElapsed
```

**It is summed by the application layer from live, non-reversed `FinalizedPayroll` records plus the Employment's `OpeningBalance`.** It is never stored as a running total, and the calculator never queries it.

### 5.6 OpeningBalance

Required so an Employer can adopt Salt mid tax year.

```text
OpeningBalance (per Employment, per TaxYear)
- PriorTaxableRemuneration
- PriorPAYE
- PeriodsElapsed
```

Without it, only employers starting on the first period of a tax year could use Salt.

---

## 6. Payroll calculation output

```text
PayrollCalculation
- EarningLines
- GrossRemuneration
- TaxableRemuneration
- PAYE            (with trace)
- EmployeeSSC     (with trace)
- EmployerSSC     (with trace)
- NetPay
- Warnings
```

### 6.1 Gross is not taxable

`GrossRemuneration != TaxableRemuneration`, and neither is derived by summing all visible pay lines. A `NonTaxableAllowance` appears in gross and not in taxable — this is the distinction PC-008 exists to prove.

### 6.2 Employer contributions are not employee deductions

Employer SSC is a payroll cost. It never reduces net pay (INV-007). The two feed different accounting entries later.

### 6.3 Explanation

Explanation is **structured data, never preformatted text**. Earnings and deductions are plain typed lines. PAYE and SSC additionally carry a small typed trace, because they are the two numbers people argue about.

```text
PAYETrace
- YearToDateTaxableRemuneration
- BandApplied
- YearToDateTaxOwed
- PAYEAlreadyWithheld
- PAYEThisPeriod

SSCTrace
- BasicWageBase
- FloorApplied?
- CeilingApplied?
- Rate
- Contribution
```

No free-text formula strings. The UI and the payslip render from this data.

### 6.4 Warnings versus errors

**Errors** stop the calculation and return `PayrollError`:

- no applicable ruleset, or overlapping rulesets;
- invalid or straddling pay period;
- missing `YearToDateContext`;
- unsupported employment or pay arrangement (concurrent employments, non-monthly basis, unsupported earning kind, unsupported deduction);
- contradictory employment dates;
- `CompensationTerms` not covering the period.

**Warnings** never block review or finalization, but are **copied into the `FinalizedPayroll`**, so the audit trail shows Salt raised a flag and a human proceeded anyway. Anything that must genuinely block is an Error, not a Warning.

---

## 7. Validation seam

Small types guard local invariants; `calculate` guards everything relational.

- `Money` cannot be constructed negative.
- `PayPeriod` cannot end before it starts.
- "These `CompensationTerms` do not cover this period" needs the rules and the period together, so it is checked inside `calculate` and returned as `PayrollError`.

**No `ValidatedPayrollInput` wrapper type.** It moves the same checks somewhere less obvious without removing any of them. Likewise, no trait for the calculator: it is a concrete function until a second implementation genuinely exists. Do not introduce interfaces to imitate C# dependency-injection habits.

---

## 8. Calculation pipeline

```text
 1. Validate context (rules cover period, terms cover period, YTD present)
 2. Determine applicable CompensationTerms for the period
 3. Build earning lines, prorating BasicPay for joiners and leavers
 4. GrossRemuneration   = all earning lines
 5. TaxableRemuneration = BasicPay + TaxableAllowance
 6. PAYE                = cumulative (§8.1)
 7. Employee SSC        = rate x clamp(BasicPay, floor, ceiling)
 8. Employer SSC        = same base, employer rate
 9. NetPay              = Gross - PAYE - EmployeeSSC
10. Assemble traces and warnings
```

### 8.1 PAYE

```text
ytd_taxable   = YearToDateContext.PriorTaxableRemuneration + TaxableRemuneration
ytd_tax_owed  = annual_bands(ytd_taxable, PeriodsElapsed + 1)
paye_period   = ytd_tax_owed - YearToDateContext.PriorPAYE
```

Cumulative PAYE is self-correcting: a corrected earlier period is absorbed at the next calculation without touching the periods in between.

### 8.2 Proration

```text
factor = days employed within the PayPeriod / total calendar days in the PayPeriod
```

The denominator is the actual length of that period (28–31 days), not a fixed number. It applies to `BasicPay` only, and only for joiners and leavers.

### 8.3 Rounding

Exact decimals throughout. Round **half-up to 2 decimal places once, per output line** — each earning line, PAYE, each SSC figure, and NetPay. Intermediate arithmetic is never rounded. The rounding rule lives in `PayrollRules` and is therefore versioned and frozen into history.

---

## 9. Payroll run

The calculator answers "what is the result for this input?". The `PayrollRun` answers "what payroll process is the Employer carrying out?".

```text
Draft -> Calculated -> Reviewed -> Finalized
```

- **Membership is live while Draft, and frozen at Calculated.** Opening a Draft does not fix the employee list. Adding someone after calculation requires an explicit action that returns the run to Draft.
- **`Reviewed` is a real domain state**, carrying `reviewed_by` and `reviewed_at`. It is audit evidence. Blocking finalization on review may be an Employer option.
- **One Employment can be recalculated mid-run.** Nothing is final yet, so no reversal is involved; if the run had reached `Reviewed`, it drops back to `Calculated`.
- Off-cycle and supplementary runs are out of scope for v1.

---

## 10. Finalized payroll and corrections

Finalization freezes, per Employment (ADR-0004):

- the complete `PayrollInput`;
- the complete `PayrollCalculation`, including warnings and traces;
- the resolved `PayrollRules` **values**, not only the `RulesetId`;
- the `RulesetId`;
- the Salt version that produced it;
- who finalized it, and when.

Corrections (ADR-0002):

```text
FinalizedPayroll  --reversed by-->  Reversal
                  --replaced by-->  new FinalizedPayroll (optional)
```

- A `Reversal` may stand alone, for someone who should not have been paid.
- Corrections **do not cascade**. Later finalized periods are untouched; cumulative PAYE absorbs the difference forward.
- A finalized record is never edited.

**Reproducible means explainable, not re-runnable.** Salt shows the stored input, rules, and output. It does not keep old calculator versions alive to recompute historical payroll bit-for-bit. The stored Salt version answers the real question behind that: if a calculation bug is found, it identifies exactly which payslips were affected.

---

## 11. Payslip boundary

A Payslip is a view rendered from a `FinalizedPayroll`, never a source of truth.

The Labour General Regulations prescribe the particulars of the written statement accompanying remuneration: employer and employee details, basic wage, pay period, hours and pay by category, other remuneration and allowances, gross remuneration, deductions, and net remuneration. Finalized payroll retains enough structured data to produce a compliant statement.

Payslip presentation concerns never enter the calculator.

```text
FinalizedPayroll
      +--> Payslip
      +--> PAYE / ETX reporting
      +--> SSC reporting
      +--> Accounting export
```

---

## 12. Statutory reporting boundary

```text
CALCULATE   "What should this employee receive?"
FINALIZE    "This is what the Employer commits to."
REPORT      "What must be reported to the employee, NamRA, or SSC?"
```

NamRA requires employers to deduct employee tax and issue PAYE5 certificates, with monthly ETX reporting through ITAS. PAYE calculation is never coupled to an ETX file format — reporting formats change independently of calculation logic. No automatic filing in v1.

---

## 13. Retention and audit

Audit is immutable rows plus an append-only action log, not event sourcing (ADR-0006).

- `FinalizedPayroll` and `Reversal` rows are never updated.
- The action log records who did what, and when.
- Unfinalized calculations are working state: only the latest calculation per Employment per run is kept.
- **Salt ships no deletion of payroll history.** The Labour Act and Social Security Act each require five years; retaining more has never harmed an employer. A purge feature is built when a customer asks.

---

## 14. Domain invariants

| ID | Invariant |
|---|---|
| INV-001 | Money is exact decimal; never binary floating point. |
| INV-002 | Calculation is deterministic for the same input and rules. |
| INV-003 | No statutory result depends on an unversioned "current" rule. |
| INV-004 | Finalized payroll is immutable. |
| INV-005 | Historical payroll is explainable from stored history alone. |
| INV-006 | Gross, taxable, and net are distinct concepts, not aliases. |
| INV-007 | Employer contributions never reduce employee net pay. |
| INV-008 | Deductions require classification and authority. |
| INV-009 | Calculation performs no I/O. |
| INV-010 | A pay period is explicit; never inferred from the clock. |
| INV-011 | Applicable employment terms are explicit and effective-dated. |
| INV-012 | Unsupported cases fail visibly; Salt never guesses. |
| INV-013 | Year-to-date is summed from live records, never stored as a running total. |
| INV-014 | `CompensationTerms` begin on a pay period start date. |

---

## 15. Rust implementation constraints

Prefer: simple structs and enums; constructors only where they enforce real invariants; explicit SQL; small public APIs; `Result` for meaningful failure; domain-focused tests; ordinary Rust.

Avoid unless proven necessary: generic repository patterns; DI frameworks; trait hierarchies; traits with one implementation for mocking; macro-heavy domain frameworks; CQRS frameworks; event buses where a function call suffices; speculative multi-country abstractions; excessive generics; unnecessary lifetimes; infrastructure types in the domain.

The code should be boring enough that both humans and coding agents can modify it confidently.

---

## 16. Persistence implications

PostgreSQL with SQLx. Tables are designed in the spec, not here. Known needs:

- effective-dated `CompensationTerms`;
- `PaySchedule` per Employer;
- `OpeningBalance` per Employment per TaxYear;
- `PayrollRun` with membership frozen at `Calculated`;
- `FinalizedPayroll` — immutable, storing frozen input, output, rule values, `RulesetId`, and Salt version;
- `Reversal` — immutable;
- append-only action log;
- year-to-date read query over live `FinalizedPayroll` plus `OpeningBalance`.

Statutory rules are **not** persisted as editable rows (ADR-0003).

---

## 17. First product tracer bullet

```text
Create Employer (with PaySchedule)
  -> Create monthly salaried Employment
  -> Enter OpeningBalance if adopting mid tax year
  -> Open PayrollRun for the period
  -> Calculate
  -> Review
  -> Finalize
  -> View compliant payslip
```

In scope: one Employer; monthly salaried Employments; `BasicPay`, `TaxableAllowance`, `NonTaxableAllowance`; cumulative PAYE; employee and employer SSC; joiner and leaver proration; opening balances; reversal and replacement; compliant payslip.

Out of scope: leave; overtime; bonuses; loans; voluntary deductions; concurrent employments; non-monthly pay bases; off-cycle and supplementary runs; imports; multi-currency; desktop or offline mode; accounting integration; automatic statutory filing.

---

## 18. Calculator scenario catalogue

The calculator seam is proven before the tracer bullet is completed.

| ID | Scenario | Purpose |
|---|---|---|
| PC-001 | Ordinary monthly salaried employee | Baseline calculation |
| PC-002 | Employee below the PAYE threshold | Zero-PAYE path |
| PC-003 | Employee crossing a tax bracket | Progressive band logic |
| PC-004 | Employee crossing multiple brackets | Higher-range logic |
| PC-005 | Employment starts mid-period | Proration, joiner |
| PC-006 | Employment ends mid-period | Proration, leaver |
| PC-007 | Taxable allowance | Classification affects PAYE, not SSC |
| PC-008 | Non-taxable travel allowance | Gross differs from taxable |
| PC-009 | BasicPay above the SSC ceiling | Ceiling clamp |
| PC-010 | BasicPay below the SSC floor | Floor clamp |
| PC-011 | Mid-year adoption with OpeningBalance | Cumulative PAYE from prior totals |
| PC-012 | Second period of a tax year | Cumulative PAYE net of prior withholding |
| PC-013 | Period 26 Aug – 25 Sep across an SSC ceiling change | Period end date selects the ruleset |
| PC-014 | Period 26 Feb – 25 Mar across the tax year end | Period end date selects the tax year |
| PC-015 | Corrected earlier period absorbed forward | Cumulative PAYE self-correction |

A scenario becomes a golden test only when: the inputs are fully explicit; the applicable rule source is identified; expected values are calculated independently; and expected values are committed as literals, never generated by production code.

---

## 19. Next steps

```text
payroll-calculation.md  (this document, settled)
        |
        v
/to-spec
        |
        v
/to-tickets
        |
        v
/implement
```

No implementation agent should be expected to invent unresolved payroll policy while implementing a ticket. Where this document says a case is unsupported, the correct implementation is a visible error, not a guess.

---

## 20. Research anchors

Constraints these establish are recorded above; the links remain for tracing a rule back to its source.

**NamRA / ITAS**
- Employee Tax — https://www.itas.namra.org.na/taxes
- FAQ (ETX reporting; gross versus taxable, travel/subsistence) — https://www.itas.namra.org.na/faq
- Downloads (ETX templates) — https://www.itas.namra.org.na/download/others

**Labour law**
- Labour Act, 2007 — https://namiblii.org/akn/na/act/2007/11/eng@2023-03-15
- Labour General Regulations, 2008 (remuneration statement particulars) — https://namiblii.org/akn/na/act/gn/2008/261/eng@2017-11-15

**Social Security**
- Social Security Commission — https://www.ssc.org.na/
- Social Security Act, 1994 — https://namiblii.org/akn/na/act/1994/34/eng@2023-03-15
- Social Security General Regulations — https://namiblii.org/akn/na/act/gn/1995/198/eng@2017-11-15

**SSC rates and limits currently encoded**
- Employee 0.9%, employer 0.9%, on BasicPay.
- Floor N$500/month, ceiling N$11,000/month from 1 March 2025.
- Ceiling N$12,500/month from 1 September 2026 (Government Gazette No. 8975 / Notice No. 236).

---

## 21. North-star statement

> Salt is not a collection of payroll screens. It is a trustworthy, historically explainable payroll engine and workflow for Namibian employers. Infrastructure exists to expose and persist that model. AI agents may write much of the implementation, but they do not get to invent what payroll means.
