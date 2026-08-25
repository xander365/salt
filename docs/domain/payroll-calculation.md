# Salt — Payroll Calculation Domain Design

**Status:** Grilled and settled, then amended by the statutory conformance grill of 2026-08-24 — see the note below. Implemented for the calculation core.
**Product:** Salt
**Initial market:** Namibian SMEs
**Date:** 2026-08-21

---

## 1. Purpose of this document

This document records the settled domain design for Salt's payroll calculation core. It came out of a `/grill-with-docs` session over the pre-specification draft; the open questions in that draft have been answered and are recorded here as decisions.

Companion documents:

- `CONTEXT.md` — the glossary. Every term below is defined there.
- `docs/adr/0001`–`0008` — the decisions that were hard to reverse, with their rejected alternatives.
- `docs/domain/statutory-conformance.md` — **amends this document.** It settles which of these rules are Namibian law, which are Salt's own policy, and what nobody has published an answer to.
- `docs/conformance/` — one provenance file per shipped rule table.

> **Amended by the statutory conformance grill of 2026-08-24.** Where this document and `statutory-conformance.md` disagree, that one wins. Five changes reach into the model below: `NonTaxableAllowance` is removed (§5.3 there); `RulesetId` is replaced by a `PayeTableId` and an `SscRulesId` on separate effective-date axes, and `ruleset_for` returns an owned composed value (ADR-0007); `YearToDateContext` gains a three-valued `PriorEmployment` fact that refuses on both `Unknown` **and** `Some` while SC-OPEN-4 is open (ADR-0001); `PayrollInput` gains an `UnsupportedDeductionStatus` knowledge state that refuses on `Present` and `Unknown`; and statutory annual band arithmetic is exposed as an exact **unrounded** seam so no statutory claim depends on Salt's rounding policy. Sections below are marked where they are superseded.

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

Rules apply to a defined period. Rule changes are ordinary, not invasive. Two real examples already in scope: the SSC ceiling moved N$9,000 → N$11,000 on 1 March 2025, and N$11,000 → N$12,500 on 1 September 2026.

**Superseded in part (ADR-0007).** There is no single `RulesetId`. The PAYE table and the SSC rules resolve on independent effective-date axes, named by a `PayeTableId` and an `SscRulesId`, because the PAYE table has not moved since 1 March 2024 while the SSC ceiling moves four more times before 2029. Each also carries a `legal_effective_from` and a `payroll_effective_from`, which differ when a regulator defers an instrument's own stated date.

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

See ADR-0005. The period end day is configurable and a PayPeriod is **not** assumed to be a calendar month. 26th-to-25th is the common example, not the rule — 18th-to-17th is equally valid. A pay period belongs to the month its **end date** falls in.

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
- PayeTable   (PayeTableId, PAYEBands,               legal_effective_from, payroll_applicability)
- SscRuleset  (SscRulesId,  rate, floor, ceiling,    legal_effective_from, payroll_applicability)
- RoundingRule
```

The two halves are resolved independently by period end date and frozen together (ADR-0007). `PayrollRules` itself is no longer effective-dated and carries no id of its own; its halves carry both.

Each half carries **two** dates. `legal_effective_from` is what the instrument says; `payroll_applicability` is the interval payroll actually applies it over, and its start is `payroll_effective_from`. Resolution keys on `payroll_applicability` alone — `legal_effective_from` is carried, stored and displayed, never consulted for selection. The pair may only record a *deferral*: payroll applying a rule later than the law does. The reverse is refused by the constructors, because it would withhold under a rule that did not yet exist.

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
- UnsupportedDeductionStatus   (ConfirmedNone | Present(kinds) | Unknown)
```

`UnsupportedDeductionStatus` is a **knowledge state**, not a list. An empty list would mean either "confirmed none" or "nobody asked"; the four kinds Salt cannot calculate — approved pension, provident fund, retirement annuity, child-education policy — are refused when `Present`, and so is `Unknown`. The governing rule: Salt requires affirmative knowledge of facts that materially affect statutory calculation, and absence of data is never read as absence of the condition.

`PaySchedule` is carried inside `PayrollInput`, not passed beside it: it exists only to validate dates — that `PayrollInput.PayPeriod` is one of the periods the schedule generates, and that `CompensationTerms.EffectiveFrom` is one of their start dates (INV-014). It never selects or generates the `PayPeriod` — the caller supplies that directly, and the calculator checks it rather than replacing it. But the same `PayrollInput` must produce the same result every time (INV-002). Passing it beside the input, the way `PayrollRules` is, would let one caller-supplied schedule accept a `CompensationTerms` that another schedule rejects for the identical `PayrollInput` — a hidden second axis of determinism that finalization (§10) does not freeze.

`PayrollRules` is passed **beside** the input:

```rust
fn calculate(
    input: &PayrollInput,
    rules: &PayrollRules,
) -> Result<PayrollCalculation, PayrollError>
```

This makes the caller choose rules deliberately, and it reads clearly in tests: same input, different rules, different answer. The signature is **unchanged** by the conformance work, borrowed arguments included — the calculator performs no rule resolution and never reaches for a catalogue.

Resolution is `ruleset_for`'s job, and that seam does change. It resolves the PAYE table and the SSC ruleset from their independent catalogues and returns an **owned** composed `PayrollRules`; there is no longer a single static value to borrow (ADR-0007).

```text
paye_table_for(period_end)
                      \
                       +--> ruleset_for -> owned PayrollRules
                      /
ssc_rules_for(period_end)
```

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

**v1 supports exactly two Earning kinds:**

```text
Earning
- BasicPay
- TaxableAllowance
```

| | SSC base | PAYE base | Gross |
|---|---|---|---|
| BasicPay | yes | yes | yes |
| TaxableAllowance | no | yes | yes |

**Superseded (statutory conformance §5.3).** `NonTaxableAllowance` is **removed**, not renamed. Allowances are not generically tax-free: Schedule 2 includes them in remuneration, and NamRA Practice Note 1 of 2024 makes travel and subsistence non-PAYE only on qualifying facts, up to UN rates or a prescribed kilometre rate that has not been announced. A user-facing "non-taxable" box could not be filled in correctly by anyone. Legal classification is a separate seam upstream of the calculator, which only ever receives Earnings already classified.

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
- PeriodsElapsed          (position in the TaxYear, 0-11 — not periods worked)
- PriorEmployment         (None | Some(taxable, paye) | Unknown)
```

`PriorEmployment` is what stops a zero meaning "nobody asked" (ADR-0001). `None` proceeds; `Unknown` refuses; `Some(..)` **also** refuses, because how a new employer must treat another employer's figures is unresolved (SC-OPEN-4) — the values ride into the typed error rather than being discarded or consumed. This is a different fact from the `OpeningBalance` in §5.6, which is prior payroll for this *same* Employment and stays fully supported.

`PeriodsElapsed` stays tax-year position: counting periods *worked* instead would over-withhold from a mid-year joiner, not correct them.

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

**Not the same fact as `PriorEmployment`.** An `OpeningBalance` is prior payroll for this *same* Employment and Employer, carried in from whatever system Salt is replacing. It is fully supported. `PriorEmployment` is remuneration from a *different* Employer earlier in the same tax year, and it is refused while SC-OPEN-4 is open. The two must never be collapsed into one another.

---

## 6. Payroll calculation output

```text
PayrollCalculation
- EarningLines
- GrossRemuneration
- TaxableRemuneration
- PayeTableId                     the PAYE instrument that produced this
- SscRulesId                      the SSC instrument that produced this
- PAYE            (with trace)
- EmployeeSSC     (with trace)
- EmployerSSC     (with trace)
- Deductions                      PAYE and employee SSC only
- NetPay
- Warnings
```

The two ids are recorded independently (ADR-0007), so a historical result names exactly which two instruments produced it. They are a reference, not the record: because rules are code (ADR-0003), a `FinalizedPayroll` stores the resolved rule *values* alongside them (ADR-0004).

`Deductions` holds PAYE and employee SSC and nothing else, so `GrossRemuneration - Deductions == NetPay` holds line for line. Employer SSC is never in it (INV-007).

### 6.1 Gross is not taxable

`GrossRemuneration != TaxableRemuneration`, and neither is derived by summing all visible pay lines. A `TaxableAllowance` appears in gross and taxable but never in the SSC base — that is the distinction PC-008 now exists to prove, since `NonTaxableAllowance` is removed. Gross and taxable therefore carry equal amounts under the two v1 kinds. That is an arithmetic coincidence of the current kinds, not an identity, and the two remain separate accumulators: a legally-named kind added later that Schedule 2 excludes from remuneration would feed gross without feeding taxable.

### 6.2 Employer contributions are not employee deductions

Employer SSC is a payroll cost. It never reduces net pay (INV-007). The two feed different accounting entries later.

### 6.3 Explanation

Explanation is **structured data, never preformatted text**. Earnings and deductions are plain typed lines. PAYE and SSC additionally carry a small typed trace, because they are the two numbers people argue about.

```text
PayeTrace
- PriorTaxableRemuneration          the OpeningBalance axis, this same Employment
- PriorPAYE                         likewise
- ThisPeriodTaxableRemuneration
- YearToDateTaxableRemuneration
- YearToDateTaxOwed                 exact and unrounded, not cents-exact
- BandsApplied                      every band crossed, with its scaled threshold and its tax
- PeriodsElapsed                    position in the TaxYear, not periods worked

SscTrace
- BasicPay                          the basic wage the lines produced
- Base                              BasicPay clamped — the amount actually charged
- Clamp                             None | Floor | Ceiling
- Rate
- Floor
- Ceiling
```

`BandsApplied` is a list, not one band: a cumulative calculation ordinarily crosses several, and a reviewer explaining a figure needs each one's contribution. `YearToDateTaxOwed` is deliberately the only non-`Money` figure in either trace — it is the exact unrounded value, before Salt's rounding policy touches it (§8.3), which is what lets a reviewer explain a PAYE figure without rerunning anything.

The PAYE amount for the period is not in the trace; it is the result beside it, and it equals `YearToDateTaxOwed - PriorPAYE`, rounded.

No free-text formula strings. The UI and the payslip render from this data.

### 6.4 Warnings versus errors

**Errors** stop the calculation and return `PayrollError`:

- no applicable PAYE table, or overlapping PAYE tables;
- no applicable SSC ruleset, or overlapping SSC rulesets — a separate variant, so a message can name which axis failed;
- invalid or straddling pay period;
- a `PayrollRules` half that does not cover the period end date — one refusal per axis, so a caller bypassing `ruleset_for` learns which half is wrong;
- a `YearToDateContext` naming a different `TaxYear` than the period end date falls in;
- prior employment status `Unknown` — the fact was never established;
- prior employment `Some(..)` — treatment unconfirmed (SC-OPEN-4), carrying the recorded figures;
- unsupported deduction status `Unknown`, or `Present` — naming every kind seen;
- unsupported employment or pay arrangement (concurrent employments, non-monthly basis, unsupported earning kind);
- contradictory employment dates;
- `CompensationTerms` not covering the period.

Every one of these is a typed variant carrying what a caller needs to act on. `Display` may be human-friendly; the domain contract is the type, never a string.

There is deliberately **no** "missing `YearToDateContext`" error. The context is a required field of `PayrollInput`, so the state is not representable and there is nothing to refuse (ADR-0001). The first period of adoption passes explicit zeros.

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
 1. Validate context (each rules half covers the period end date, the YTD context names the period's TaxYear, unsupported deductions are ConfirmedNone, prior employment is None, period is on the Employer's PaySchedule, terms cover every day employed)
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
ytd_tax_owed  = bands_scaled_to(PeriodsElapsed + 1, of 12) applied to ytd_taxable
paye_period   = round(ytd_tax_owed - YearToDateContext.PriorPAYE)
```

It is the band **thresholds** that are scaled to `(PeriodsElapsed + 1) / 12`, never the income. `PeriodsElapsed` is position in the tax year — see §5.5.

**This whole step is Salt policy, not conformance** (SC-OPEN-1). The band table is statutory; deriving one period's figure from it is not. The same band walk entered *without* scaling is the statutory seam, and that one takes `Money` and returns an exact unrounded value.

Cumulative PAYE is self-correcting: a corrected earlier period is absorbed at the next calculation without touching the periods in between.

### 8.2 Proration

```text
factor = days employed within the PayPeriod / total calendar days in the PayPeriod
```

The denominator is the actual length of that period (28–31 days), not a fixed number. It applies to `BasicPay` only, and only for joiners and leavers.

A fully employed period is **not** divided and re-multiplied. `BasicPay` is carried through untouched, so twelve consecutive unprorated periods sum to exactly twelve months' pay with no rounding drift.

The `CompensationTerms` must be in force for every day **being paid for**, not for every day of the period. For a continuing employee those are the same span; for a leaver they are not, and terms that end on the employee's last day are the ordinary, correct record — refusing them would make a correctly recorded leaver uncalculable. Terms that stop *before* the last day employed are still refused: those days are unpriced, and Salt does not guess (INV-012).

Proration divides by the length of the supplied `PayPeriod`, so that period must be one the Employer's `PaySchedule` actually generates. A period the schedule does not produce is refused, naming the period the schedule runs around that start date. Without this, INV-014 would be checked against a schedule the period itself does not follow.

### 8.3 Rounding

Exact decimals throughout. Round **half-up to 2 decimal places once, per output line** — each earning line, PAYE, each SSC figure, and NetPay. Intermediate arithmetic is never rounded. The rounding rule lives in `PayrollRules` and is therefore versioned and frozen into history.

**Rounding is Salt policy (SC-OPEN-2), and the seams keep it out of statutory arithmetic.** Statutory bands applied to a cents-exact amount can yield fractions of a cent, so the statutory annual-tax seam takes `Money` and returns an exact unrounded value; the rounding rule is applied only afterwards, inside the calculator. A statutory table test therefore never asserts the rounding policy, and changing that policy cannot invalidate one.

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
- the resolved `PayrollRules` **values**, not only their ids;
- the `PayeTableId` and the `SscRulesId`;
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

INV-012 is the one the conformance work leaned on hardest: an unsupported deduction, an unestablished prior-employment fact, and a recorded prior-employment figure whose treatment is unconfirmed are all **refusals**, never approximations. Absence of data is never read as absence of the condition.

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
- `FinalizedPayroll` — immutable, storing frozen input, output, rule values, `PayeTableId` and `SscRulesId`, and Salt version;
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

In scope: one Employer; monthly salaried Employments; `BasicPay` and `TaxableAllowance`; cumulative PAYE; employee and employer SSC; joiner and leaver proration; opening balances; reversal and replacement; compliant payslip.

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
| PC-008 | Taxable allowance beside a baseline payroll | The SSC base is independent of gross and taxable |
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
