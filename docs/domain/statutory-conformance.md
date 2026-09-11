# Salt — Namibian Statutory Conformance

**Status:** Grilled, specified and implemented. Specified as GitHub issue #7; delivered by issues #8–#16. Authoritative: where the code disagrees with this document, the code is the defect.
**Product:** Salt
**Market:** Namibia
**Date:** 2026-08-24
**Supersedes:** the pre-specification draft of 2026-08-21
**Primary target:** the payroll calculation core implemented from GitHub issue #1

---

## 1. Purpose of this document

Salt's first design document settled a software question:

```text
PayrollInput + PayrollRules -> PayrollCalculation
```

That seam is pure, deterministic, serializable and effective-dated. It does not, by itself, prove that the rule values inside it are correct for Namibia. The implementation proved the risk: synthetic PAYE brackets written to test the progressive-band algorithm were also shipped as the production Namibian table.

Salt therefore runs two separate controls:

```text
SPEC CONFORMANCE       Does the code implement our agreed design?
STATUTORY CONFORMANCE  Does our agreed design and shipped rule data agree with Namibia?
```

This document is the second control. It records what the law actually says, what Salt has decided where the law is silent, and — explicitly — which of Salt's answers are not law at all.

Companion documents:

- `CONTEXT.md` — the glossary.
- `docs/adr/0001`–`0008` — the decisions, with their rejected alternatives.
- `crates/payroll/docs/conformance/` — one provenance file per shipped rule table, inside the pure crate beside the code it proves (ADR-0009).

### The distinction this document exists to enforce

Three claims are not the same claim, and Salt keeps them apart everywhere — in this document, in the ADRs, and in the test names:

| Claim | Meaning | Test prefix |
|---|---|---|
| **Statutory** | Traceable to a Tier A source. Salt implements it. | `statutory_*` |
| **Provisional Salt policy** | The law is silent or unpublished. Salt chose. Revisit on confirmation. | `salt_policy_*` |
| **Algorithm** | Synthetic values proving mechanics. Never reachable in production. | `algorithm_*` |

Calling a policy decision "statutory conformance" is the failure mode this document is designed to prevent.

---

## 2. Rule-source hierarchy

Sources are not equal.

**Tier A — primary law / regulator.** Namibian statutes and amendments; Government Gazette and Government Notices; NamRA official publications, practice notes, forms and guidance; Social Security Commission official guidance. A rule contradicted by Tier A is not saved by vendor behaviour.

**Tier B — professional interpretation.** PwC Namibia; established Namibian tax and accounting professionals; payroll-vendor statutory release notes for Namibia. Most useful where written law and regulator operational practice diverge.

**Tier C — implementation cross-checks.** Sage Namibia documentation and calculators; other established Namibian payroll products; reputable calculators. A rule is not law because several products implement it.

Where a legal instrument and an operational instruction differ, Salt records **both**.

---

## 3. Statutory findings — Tier A backed

Everything in this section is traceable to a Tier A source and may be implemented as law.

### 3.1 Individual income-tax table

Effective **1 March 2024**, still the current table for the 2026/27 year of assessment.

| Annual taxable income | Tax |
|---|---:|
| N$0 – N$100,000 | No tax |
| N$100,001 – N$150,000 | 18% of the amount above N$100,000 |
| N$150,001 – N$350,000 | N$9,000 + 25% of the amount above N$150,000 |
| N$350,001 – N$550,000 | N$59,000 + 28% of the amount above N$350,000 |
| N$550,001 – N$850,000 | N$115,000 + 30% of the amount above N$550,000 |
| N$850,001 – N$1,550,000 | N$205,000 + 32% of the amount above N$850,000 |
| Above N$1,550,000 | N$429,000 + 37% of the amount above N$1,550,000 |

Verified internally consistent: each band's base amount equals the cumulative tax at the band below it (`9,000 = 18% × 50,000`; `59,000 = 9,000 + 25% × 200,000`; and so on through `429,000`).

Sources: `crates/payroll/docs/conformance/paye-2024-03.md`.

**Note on the NamRA brochure.** The brochure's sixth row prints "Exceeds N$ 800 000". Every other source, and the table's own internal arithmetic, gives **N$ 850,000**. Salt implements N$850,000 and records the discrepancy here.

**Current Salt state — closed.** Salt used to ship this synthetic table in production:

```text
0 -> 0%   120,000 -> 20%   240,000 -> 30%   480,000 -> 40%
```

That was the blocker this document was written to end, and it is fixed. The table above is now the shipped `paye-2024-03` catalogue entry, and the synthetic values survive only as test fixtures — asserted unreachable through the public resolvers by `algorithm_synthetic_paye_bands_are_unreachable_through_production_resolvers`. The history stays recorded here because ADR-0008 exists because of it.

### 3.2 Individual tax year

```text
1 March -> last day of February
```

### 3.3 Social security contribution base

The Social Security General Regulations define `basic wage` as remuneration for ordinary work, **excluding** allowances (travel, subsistence, housing, motor vehicle, transport, professional), overtime, additional Sunday and public-holiday pay, additional night-work pay, and pension, annuity, medical or insurance benefits. The Labour Act definition is closely aligned.

Salt's base is therefore:

```text
SSC base = BasicPay        (never GrossRemuneration, never TaxableRemuneration)
```

This is sound **only** so long as `BasicPay` really carries the statutory `basic wage`. Every future earning kind must be classified explicitly against this base.

### 3.4 Social security rates, floor and ceiling

```text
employee rate: 0.9%      employer rate: 0.9%
minimum basic wage: N$500/month  ->  N$4.50 per side
```

Ceiling schedule:

| Payroll application | Maximum basic wage | Max contribution per side |
|---|---:|---:|
| Through August 2026 | N$11,000 | N$99.00 |
| From September 2026 | N$12,500 | N$112.50 |
| From March 2027 | N$14,000 | N$126.00 |
| From March 2028 | N$15,000 | N$135.00 |
| From March 2029 | N$16,000 | N$144.00 |

**Legal date versus operational date.** Government Notice 236 (Gazette 8975) states an effective date of **1 March 2026** but was only gazetted on **15 July 2026**. The SSC confirmed the increase is implemented from the **September 2026** payroll, with no retrospective March–August adjustment. Salt stores both dates — see §5.4 and ADR-0007.

Sources: `crates/payroll/docs/conformance/ssc-2026-09.md` and the other SSC provenance files.

### 3.5 Deductions Salt does not support

Schedule 2 allows certain current contributions to reduce remuneration for PAYE, subject to limits. NamRA's brochure names four:

- approved pension fund contributions made as a condition of employment;
- provident fund contributions;
- retirement annuity fund contributions;
- premiums on a policy taken out for the education of a child.

Their **combined** deduction is capped at **N$150,000** per tax year (from the 2023 tax year).

Salt v1 supports none of them. That is acceptable only because Salt **refuses** such an employee rather than calculating them as though the deduction did not exist. See §5.5.

A fifth, unrelated fact is refused through the same machinery for a different reason: **employer-paid** medical aid is not one of these four deductions at all, but a fringe benefit whose taxable value Salt cannot compute (Q-OPEN-9). It is grouped with the four above because both kinds block `calculate` the same way, not because both are deductions — see §5.11 for the **employee's own** medical aid premium, which Salt does support.

### 3.6 Allowances are not generically tax-free

Schedule 2 defines remuneration broadly and includes allowances. A reimbursement wholly for expenditure actually incurred in the course of employment is excluded from remuneration.

NamRA Practice Note 1 of 2024 distinguishes qualifying business travel and subsistence from an ordinary allowance forming part of remuneration:

- qualifying subsistence up to applicable UN travel rates is not subject to PAYE;
- the excess above the permitted amount is subject to PAYE;
- qualifying business-travel reimbursement at the prescribed rate can be non-PAYE;
- the excess over the prescribed rate is subject to PAYE;
- the Practice Note does not apply to an allowance paid as part of ordinary remuneration.

The prescribed kilometre rate had not been announced in the material reviewed.

The NamRA brochure adds two further classifications Salt does not model: housing allowance is taxable **except one third, if the employer has an approved housing scheme**; and use of a company car is taxed at 1.5% (employer pays all costs) or 1.4% (employee pays fuel).

The consequence is §5.3: Salt v1 ships no user-facing "non-taxable" box at all.

---

## 4. Open statutory questions — NEEDS CONFIRMATION

No Tier A source was found for any of the following. Each must be revisited when an answer arrives.

Six of the eight have a Salt policy attached in §5. **SC-OPEN-4 and Q-OPEN-22 deliberately do not** — each has a refusal. Where Salt cannot even state what the rule would be, inventing one more unverified rule is worse than stopping.

| # | Question | Salt's response | Stamp |
|---|---|---|---|
| SC-OPEN-1 | How is one period's PAYE derived from the annual table? No prescribed deduction tables were found published. | Policy (§5.1) | `NEEDS NAMRA CONFIRMATION` |
| SC-OPEN-2 | Is there a prescribed rounding behaviour for PAYE or SSC? | Policy (§5.8) | `NEEDS NAMRA CONFIRMATION` |
| SC-OPEN-3 | Do the SSC floor and ceiling apply in full to a part-month joiner or leaver? | Policy (§5.7) | `NEEDS SSC CONFIRMATION` |
| SC-OPEN-4 | How must a new employer treat prior-employer remuneration and PAYE, and is a directive or certificate required first? | **Refusal** (§5.6) | `NEEDS NAMRA CONFIRMATION` |
| SC-OPEN-5 | One older source (US SSA country profile, 2019) gives the SSC minimum earnings base as N$300, not N$500. Salt implements N$500, consistent with current reporting, the shipped code and the published N$4.50 minimum contribution. | Implemented as N$500 | `flagged` |
| SC-OPEN-6 | What divisor converts a monthly salary to an hourly rate for overtime? No published Namibian rule prescribing one was found. | Policy (§5.10) | `NEEDS CONFIRMATION` |
| SC-OPEN-7 | Does Salt correctly grant no relief against taxable income for an employee's own medical aid premium? NamRA's four allowed deductions (§3.5) do not list it, but no primary source was read confirming that absence is deliberate. | Policy (§5.11) | `NEEDS NAMRA CONFIRMATION` |
| Q-OPEN-22 | What does Namibian law require when a voluntary deduction (e.g. a medical aid premium) exceeds the net pay available to withhold it from — a priority order, a cap, a protected-earnings floor, carry-forward? | **Refusal** (§5.11) — Salt invents none of these and names the shortfall instead | `NEEDS NAMRA CONFIRMATION` |

**SC-OPEN-1 is a real choice, not a gap.** Sage ships two methods for Namibia: **Normal Tax**, which annualises the current period, and **Average Tax**, which uses year-to-date income based on time worked. Salt implements a third shape (§5.1). The Income Tax Act anticipates deduction tables and methods prescribed by the Minister; it does not define Salt's formula. Nothing here entitles Salt to call its per-period arithmetic statutory.

---

## 5. Provisional Salt policy — decided, not law

Everything in this section is Salt's decision. Each is defensible, each is tested, and none of it may be described as statutory conformance.

### 5.1 PAYE per-period method — cumulative scaled thresholds

Salt calculates PAYE cumulatively: the annual band **thresholds** are scaled to `period_number / 12`, and the un-annualised year-to-date taxable remuneration is taxed against those scaled thresholds. PAYE for the period is that figure less PAYE already withheld this tax year.

Rejected: annualising the current period (`month × 12`, tax, `÷ 12`), which is Sage's Normal Tax and the common local vendor behaviour.

Why: it reconciles exactly to the standard annual calculation at period 12, absorbs irregular remuneration without special cases, and self-corrects after a correction — which is what makes ADR-0002 cheap.

Recorded in ADR-0001, amended to state plainly that this is Salt's choice under an unprescribed method. Stamped SC-OPEN-1.

### 5.2 `PeriodsElapsed` counts tax-year position

`PeriodsElapsed` is the employee's position in the **tax year**, 0–11. It is **not** a count of periods the Employment has been paid, and **not** a count of days worked.

This is Salt policy, not conformance. It has no stamp of its own because it is not a separate open question: it is an inseparable part of the per-period method stamped SC-OPEN-1, and it stands or falls with it. Any test asserting a period's PAYE under this interpretation is `salt_policy_*`.

The draft treated this as the highest risk in the system, on the belief that a new starter would be materially under-taxed. Worked through, that is not what happens. A person starting in October on N$25,000 a month earns N$125,000 in the tax year; true annual liability is N$4,500. In October the thresholds are scaled to 8/12, so N$25,000 falls under the band and PAYE is N$0 — but by February the thresholds are whole and the full N$4,500 has been collected. The method self-corrects, and it reaches the correct annual figure.

Counting *worked* periods instead would scale October's thresholds to 1/12, tax the new starter immediately, and **over**-withhold against their true annual liability — the failure this paragraph exists to prevent, and the reason the code carries the same warning at the type itself. A reader who "fixes" `PeriodsElapsed` into periods worked does not correct an under-taxation bug; they create an over-withholding one.

The genuine hole is different, and §5.6 closes it: someone who already earned taxable remuneration elsewhere in the same tax year.

### 5.3 v1 earning kinds

```text
BasicPay
TaxableAllowance
Overtime
```

`Overtime` was added by issue #76 (ADR-0022). It is priced from hours and a multiplier, never typed as money, and it feeds `GrossRemuneration` and `TaxableRemuneration` but **never** the social security base — that is §3.3 applied, settled law and not a Salt choice. The divisor that prices it *is* a Salt choice, and is §5.10.

`NonTaxableAllowance` is removed, not renamed. It let a user tick "non-taxable" on a travel allowance without any of the legal qualifying facts (§3.6), and the prescribed kilometre rate is not even published yet.

Legal classification does **not** live in the calculator. The calculator receives already-classified `Earning`s. Turning a raw pay line into a legal category needs trip facts, versioned UN rates, a kilometre rate and human judgement — that is a separate seam, upstream, and not part of this work.

Adding legally-named kinds later (`QualifyingSubsistence`, `BusinessTravelReimbursement`, housing with an approved scheme) is additive. Shipping a wrong tax-free box is not recoverable.

### 5.4 Effective dates — PAYE and SSC on separate dials

One `PayrollRules` value with one effective period forced a fictitious new "PAYE version" every time only the SSC ceiling moved.

PAYE rules and SSC rules now resolve on **independent** date axes and freeze into one combined `PayrollRules` value handed to `calculate()`. The calculator does not change.

There is no single combined `RulesetId` — a combined identity changes when only one half moved, which is the problem this solves. `PayrollRules` carries a **`PayeTableId`** and an **`SscRulesId`**, each with its own effective period and its own provenance document, and a stored calculation records both.

`ruleset_for` resolves both catalogues by period end date and returns an **owned** composed `PayrollRules`, since there is no longer one static value to borrow. `calculate` keeps its existing borrowed seam and performs no resolution of its own. Resolution failures are typed per catalogue — a missing or overlapping PAYE table is a different error from a missing or overlapping SSC ruleset, so a message can say which axis is wrong.

Each table carries two dates:

```text
legal_effective_from       what the instrument says
payroll_effective_from     what payroll actually applies
```

For the September 2026 SSC ceiling these differ (1 March 2026 versus 1 September 2026), and the difference is a genuine calculation input, not a footnote. Source URLs and prose stay in `crates/payroll/docs/conformance/`, not in the pure calculation value.

Recorded in ADR-0007.

### 5.5 Unsupported deductions are refused, never approximated

`PayrollInput` carries an explicit **knowledge state** about the facts that would change PAYE which Salt cannot calculate (§3.5) — the four unsupported deduction kinds, plus employer-paid medical aid (issue #78):

```text
UnsupportedDeductionStatus
- ConfirmedNone        established: this employee has none
- Present(kinds)       non-empty; the kinds seen
- Unknown              nobody established it
```

`ConfirmedNone` proceeds. `Present(kinds)` refuses with a typed error **naming every kind**. `Unknown` refuses, because the fact was never established.

A bare list would not be enough: an empty list would mean either "confirmed none" or "nobody asked", which is the same ambiguity §5.6 exists to remove. `Present` is built through a constructor that rejects an empty collection, and `Option<Vec<..>>` is rejected for the same reason — it moves the ambiguity rather than removing it.

A pure library cannot refuse an input it has no field for; silence is all it could offer, and silence is the dangerous outcome. The employer must state the fact, and Salt must stop.

The N$150,000 combined annual cap is recorded (§3.5) and not implemented. Salt refuses these deductions; it does not calculate them.

**Naming.** `StatutoryDeduction` already exists in the code and means a PAYE or SSC amount actually withheld. The new type must not reuse that name.

Preserve: **unsupported real-world payroll profiles are rejected, never approximated.**

### 5.6 Two different facts: OpeningBalance and PriorEmployment

These are not the same fact and must never be conflated.

| Fact | Meaning | Treatment |
|---|---|---|
| **`OpeningBalance`** | Prior year-to-date figures for **this same Employment and Employer**, from before Salt — a mid-year system replacement. | **Supported**, unchanged (ADR-0001). |
| **`PriorEmployment`** | Taxable employment with **another Employer** earlier in the same tax year. | **Refused** while SC-OPEN-4 is open. |

`OpeningBalance` is how `YearToDateContext`'s prior taxable remuneration, prior PAYE and periods elapsed get their values when an employer adopts Salt mid-year. That is settled Salt policy and is unaffected by anything below.

`PriorEmployment` is a separate, explicitly three-valued fact on `YearToDateContext`:

```text
None                    confirmed: no earlier taxable employment this tax year
Some(taxable, paye)     figures from the employee's tax certificate
Unknown                 nobody established the fact
```

Behaviour:

- `None` → calculation proceeds under Salt's documented per-period policy.
- `Unknown` → **refuse**. Zero must stop meaning "we did not ask" — that conflation is the actual under-taxation risk, not §5.2.
- `Some(..)` → **refuse**. How a new employer must treat prior-employer remuneration and PAYE, and whether a NamRA directive or certificate is required first, is SC-OPEN-4 and unresolved. Consuming the figures would turn an open question into shipped payroll behaviour, which is the exact failure this document exists to end.

This is **not** a Salt policy decision. There is no policy for it; there is a refusal. The figures stay in the `Some` variant and travel into the typed error rather than being discarded, so nothing has to be re-gathered once the treatment is confirmed.

### 5.7 Part-month SSC — full monthly floor and ceiling

`BasicPay` is prorated by actual calendar days in the period. The result is then clamped to the **full monthly** floor and ceiling — never a prorated one.

A person who works one day and earns N$200 basic wage still contributes N$4.50 per side.

The regulations state the floor and ceiling as monthly amounts, and ADR-0005 already refuses to split monthly statutory amounts pro-rata. Being consistent with Salt's own written rule beats inventing a new one. Stamped SC-OPEN-3.

Multiple employments are out of scope: the calculator only ever sees one Employment.

### 5.8 Rounding

Full decimal precision inside the calculation. Half-up to cents at each statutory output: PAYE, employee SSC, employer SSC.

It matches the N$4.50 and N$112.50 figures the SSC itself publishes, and half-cent amounts are not payable. Stamped SC-OPEN-2.

**Rounding is not part of the statutory arithmetic, and the seam must show it.** The statutory bands applied to a cents-exact taxable amount can produce fractions of a cent — N$0.01 above a threshold at 18% is exactly N$0.0018. A statutory annual-tax function that returned `Money` would therefore be forcing an unconfirmed Salt policy inside a seam claiming to be law. It returns an exact unrounded value instead:

```text
       taxable remuneration        (Money — non-negative, cents-exact)
              |
              v
statutory annual band arithmetic   (exact, unrounded)
              |
              v
        exact tax value
              |
              v
     Salt rounding policy          (SC-OPEN-2)
              |
              v
            Money
```

### 5.9 PAYE refunds

Unchanged from ADR-0001. If a correction lowers recalculated year-to-date liability below PAYE already withheld, `calculate()` refuses rather than returning a refund or clamping to zero. Refunds are not modelled in this domain.

### 5.10 Salary to hourly rate for overtime

Overtime is typed as **hours at a multiplier** and priced by a rate Salt derives:

```text
DerivedHourlyRate = BasicPay x 12 / 52 / OrdinaryHours
```

`BasicPay` is the **contractual** figure on the `CompensationTerms` row, never the prorated one: a person's hourly rate does not fall because they joined mid-month. `OrdinaryHours` is the agreed weekly hours recorded on that same row.

No published Namibian rule prescribing a divisor was found. The whole formula is Salt's decision and **must never be described as law**. Stamped SC-OPEN-6, `NEEDS CONFIRMATION`.

`OrdinaryHours` is recorded per Employment precisely so the assumption is visible and dated, rather than hidden in a constant such as 173.33. An overtime line on a period whose terms row records no `OrdinaryHours` is **refused**; a salary-only period on that same row still pays, because missing hours are unknown, not invalid.

The rate is an exact reduced fraction and is never rounded. Hours and
multiplier are applied to that fraction, and there is exactly **one** rounding,
at the line, through §5.8's seam. Two lines at the same multiplier stay two
lines and round independently.

The stamp travels as data, not prose: every `OvertimeTrace` carries a `SaltPolicyStamp` naming `SC-OPEN-6` and `NEEDS CONFIRMATION`, so the screen renders the sentence and the calculator emits no user-facing English.

The multiplier set is closed at **1.5 and 2.0** (D31) and is an `enum`, not a configuration value. ⚠️ Whether those are the correct and only statutory factors in Namibia is **`Q-OPEN-8`** — assumed from the owner's practice, not verified. Recorded in ADR-0022.

The seam is deliberately asymmetric: it takes `Money` and returns an exact unrounded value. Taxable remuneration is a monetary domain value and must stay non-negative and cents-exact at the public boundary rather than degrading to a bare decimal; the *result* cannot be `Money` for the reason above. The conversion happens inside, before the band arithmetic.

Two consequences: changing the rounding policy must not invalidate a single statutory table test, and the calculator applies rounding only after statutory arithmetic has produced an exact value. There is still only one implementation of progressive-band arithmetic — the statutory seam is that same walk entered without threshold scaling, not a second copy.

### 5.11 Voluntary deductions — medical aid premium (issue #78)

`Deduction` gains a second branch alongside `StatutoryDeduction`: `Voluntary(VoluntaryDeduction)`. v1 constructs exactly one kind, `MedicalAidPremium` — the employee's own contribution, withheld at the amount instructed and no other. A second voluntary kind is a code change and a deliberate decision, exactly like a third overtime multiplier; there is no generic "other deduction" with a free-text type.

The premium is computed **after** PAYE and employee social security, from the earning lines exactly as if it did not exist — the earning-bases accumulator never sees it, so PAYE and both social security figures are bit-identical to the same input without the deduction. Deductions are ordered PAYE, employee social security, then voluntary — the order a payslip prints and net pay is derived in.

A voluntary deduction that would take net pay below zero is refused — `PayrollError::DeductionsExceedGrossRemuneration` extended to carry the shortfall — rather than partially withheld. Salt invents no priority order, cap, or carry-forward to resolve this; the existing checked-subtraction chain is simply extended to include voluntary deductions in the sum it checks.

Salt grants **no relief against taxable income** for the employee's own medical aid premium. NamRA's four allowed deductions (§3.5) do not list medical aid, which supports that reading, but no primary source was read confirming it. Stamped SC-OPEN-7, `NEEDS NAMRA CONFIRMATION` — every test whose expected figure depends on this reading is `salt_policy_*`, never `statutory_*` (ADR-0008).

**Employer-paid** medical aid is a different fact — a fringe benefit, not a deduction — and is refused by name through the existing `UnsupportedDeductionStatus` machinery (§5.5), widened from "the four deductions NamRA's brochure allows against taxable income" to "facts that would change PAYE which Salt cannot calculate". How that benefit would be valued for tax remains unresolved and non-blocking, because it is refused rather than guessed at (Q-OPEN-9).

What Namibian law requires when a voluntary deduction exceeds available net pay — a priority order, a cap, a protected-earnings floor, carry-forward — is unresolved. Recorded as `Q-OPEN-22`.

---

## 6. Date semantics — settled

**Authoritative, from the product owner:**

> The pay period end day is configurable. A pay period belongs to the month its **end date** falls in.

26th-to-25th was only an example; 18th-to-17th is equally valid. The pay date typically falls near the end of the month the period closes in.

The period end date therefore selects both the `PayrollRules` and the `TaxYear`. ADR-0005 stands unchanged and is now stated in the owner's own words.

**No `TaxPeriod` in v1.** PAYE is remitted within 20 days after the month in which the tax was deducted — calendar-month reporting against non-calendar pay periods. That is a **reporting** clash, not a calculation clash, and this work is calculator-only. Reversing later is cheap because nothing stored today would be wrong. `PayrollRun` and the pay date are where the reporting month will eventually come from.

---

## 7. Test taxonomy

**Three prefixes**, one per claim in §1, and they may not be mixed:

| Prefix | Claim | Meaning |
|---|---|---|
| `statutory_*` | Statutory | The expected value is a literal published by a regulator. Traceable to a Tier A source, and the only kind of case an evidence entry may cite. Splits into two families by instrument: `statutory_paye_*` and `statutory_ssc_*`. |
| `salt_policy_*` | Provisional Salt policy | Salt chose, because the law is silent or the method is unpublished. Defensible, tested, revisited on confirmation — and never citable as conformance evidence. |
| `algorithm_*` | Algorithm | Synthetic values proving mechanics. Makes no claim about Namibia at all, and must stay unreachable through any public production resolver. |

A test that carries none of the three prefixes makes no claim about a calculated outcome: it exercises a type's own construction, validation or serialization — `Money` rejecting a negative, `PeriodsElapsed` rejecting a thirteenth period, a shipped value round-tripping through `serde`. The moment a test asserts what an employee is paid, withheld or contributes, it takes a prefix, and which prefix it takes is a statement about what Salt is claiming.

Test names carry **human** semantics. They are not the compliance mechanism — the evidence catalogue below is.

### `statutory_paye_*` — the annual table only

Exact, unrounded annual band arithmetic, traceable to NamRA. These cases must not construct a `PayrollInput`, a `YearToDateContext` or a `PeriodsElapsed`, must not run the cumulative per-period method, and must not exercise the rounding rule (§5.8). Boundaries:

```text
SC-PAYE-001    N$100,000   -> N$0
SC-PAYE-002    N$150,000   -> N$9,000
SC-PAYE-003    N$350,000   -> N$59,000
SC-PAYE-004    N$550,000   -> N$115,000
SC-PAYE-005    N$850,000   -> N$205,000
SC-PAYE-006    N$1,550,000 -> N$429,000
SC-PAYE-007    N$1,650,000 -> N$466,000
```

Inside brackets:

```text
SC-PAYE-008    N$120,000   -> N$3,600
SC-PAYE-009    N$250,000   -> N$34,000
SC-PAYE-010    N$450,000   -> N$87,000
SC-PAYE-011    N$700,000   -> N$160,000
SC-PAYE-012    N$1,000,000 -> N$253,000
SC-PAYE-013    N$2,000,000 -> N$595,500
```

Every expected value is a literal from NamRA's table, never a computed expression. All happen to be exact to the cent, which is what makes them usable as statutory cases even though an arbitrary taxable amount can produce sub-cent exact tax.

### `statutory_ssc_*` — rate, floor, ceiling amounts

```text
SC-SSC-001     low basic wage        -> N$4.50 per side
SC-SSC-002     high earner, Aug 2026 -> N$99.00 per side
SC-SSC-003     high earner, Sep 2026 -> N$112.50 per side
SC-SSC-004     high earner, Mar 2027 -> N$126.00 per side
SC-SSC-005     high earner, Mar 2028 -> N$135.00 per side
SC-SSC-006     high earner, Mar 2029 -> N$144.00 per side
SC-SSC-007     a TaxableAllowance does not increase the SSC base
```

Part-month clamping is **not** a statutory case while SC-OPEN-3 is open.

### `salt_policy_*` — anything asserting a period's outcome

Any test of the form "this period's PAYE is X" belongs here, because the method is unconfirmed (SC-OPEN-1):

- `salt_policy_paye_*` — new starter in October; same-employer mid-year `OpeningBalance` adoption; a correction absorbed by the next period; `PriorEmployment::Unknown` refuses; `PriorEmployment::Some` refuses pending SC-OPEN-4, with the figures surviving into the error; `UnsupportedDeductionStatus::Unknown` refuses; `Present(kinds)` refuses and names them.
- `salt_policy_ssc_*` — part-month joiner and leaver clamping to the full monthly floor and ceiling (SC-OPEN-3).
- `salt_policy_rounding_*` — half-up at each statutory output (SC-OPEN-2).
- `salt_policy_overtime_*` — anything whose figure depends on the salary-to-hourly divisor (SC-OPEN-6, §5.10): the derived rate, the money on a line, that the rate comes from contractual and not prorated pay, and that an `OrdinaryHours` change dated mid-period is refused by the existing rules. **Never `statutory_*`** — a `statutory_*` name means a literal published by a regulator and is citable as evidence under ADR-0008, and no regulator published this divisor. Overtime *mechanics* that hold whatever the divisor is — that two lines round independently, that a multiplier outside the closed set is refused, that overtime never reaches the social security base — are `algorithm_*`.
- `salt_policy_*` for a medical aid premium (SC-OPEN-7, §5.11) — that it reduces net pay by exactly its amount, that PAYE and both social security figures are unchanged, that it appears as its own classified line after the statutory ones, and that one exceeding available net pay is refused naming the shortfall rather than partially withheld (Q-OPEN-22). **Never `statutory_*`** — Salt's no-relief reading is unconfirmed.
- **Ruleset selection by period end date** — that a period straddling a rate change uses the entry covering its **end date**, for its whole length. The rule is ADR-0005's, stated by the product owner; no Namibian source prescribes it. What is statutory is the ceiling value and its instrument's effective date. How Salt maps a pay period onto that date is Salt policy, so a straddle test is `salt_policy_*` and may never be cited as evidence. The N$11,000-to-N$12,500 straddle across 1 September 2026 is the live example.

### `algorithm_*` — synthetic mechanics

The `0 / 120k / 240k / 480k` fixtures are `algorithm_*` values. Every test that asserts something *about the fixtures themselves* — that the band walk sums correctly, that they never appear in a shipped catalogue, that no public resolver returns them — is `algorithm_*`, and they must be unmistakably synthetic and unreachable through any public production resolver.

They may also serve as the rules fixture underneath a `salt_policy_*` test, and that is not a leak. A test of Salt's per-period method, its rounding, or its proration is asserting the *method*, and round synthetic thresholds make the arithmetic legible in a way the real table does not. What matters is the direction of the claim: a synthetic value may back a Salt-policy claim about mechanics, and may never back a statutory claim about Namibia. Only the statutory case catalogue can do that, and an evidence entry can reference nothing else.

### The conformance gate — explicit evidence

A shipped `PayeTableId` or `SscRulesId` must have **both** a provenance document in `crates/payroll/docs/conformance/` and at least one statutory golden case, or **the verification suite fails** — `cargo test`, and CI with it. Not `cargo build`.

The mechanism is an explicit evidence catalogue, not a scan of source files or test names. Renaming a test must not be able to break or satisfy a compliance check:

```text
ConformanceEvidence {
    rule_id:             "ssc-2026-09",
    provenance_document: "docs/conformance/ssc-2026-09.md",
    statutory_case_ids:  ["SC-SSC-003"],
}
```

The `SC-PAYE-*` and `SC-SSC-*` cases above are themselves explicit data, and the statutory tests iterate that catalogue — so the evidence registry points at something real. The suite proves, for every shipped id: an evidence entry exists; its provenance document exists on disk; it names at least one case id; every id it names exists in the case catalogue; and nothing outside the statutory case catalogue can satisfy the requirement.

Recorded in ADR-0008. This is the control that would have caught the synthetic table.

---

## 8. Independent cross-product checks

Once the per-period method is confirmed, compare Salt against at least one independent Namibian payroll calculator for: first period, later stable period, new starter mid-tax-year, joiner part-period, leaver part-period, taxable allowance, and a correction/YTD recalculation.

The external calculator is a cross-check, not a source of law.

---

## 9. Work this settles

Specified as GitHub issue #7, delivered as issues #8–#16. Each numbered item below is shipped; the list stays as the record of what the work covered, not as a plan.

1. Replace the synthetic production PAYE bands with the real table (§3.1).
2. Keep the synthetic bands as `algorithm_*` fixtures only, unreachable through any public resolver.
3. Add the SSC ceiling schedule through March 2029 (§3.4).
4. Split PAYE and SSC onto independent effective-date axes; `PayeTableId` and `SscRulesId` replace the single `RulesetId` (§5.4). `ruleset_for` resolves both and returns an **owned** composed value; `calculate` keeps its existing borrowed seam and performs no resolution.
5. Add `legal_effective_from` and `payroll_effective_from` to each table.
6. Add a `docs/conformance/` provenance document per table.
7. Expose a statutory annual-tax seam returning an **exact unrounded** value, sharing one band-arithmetic implementation with the per-period path (§5.8).
8. Remove `NonTaxableAllowance` (§5.3).
9. Add `UnsupportedDeductionStatus` and its two refusals (§5.5).
10. Add `PriorEmployment` and its two refusals, kept distinct from `OpeningBalance` (§5.6).
11. Split the ruleset resolution errors per catalogue, so a message can say which axis failed.
12. Add the `statutory_*`, `salt_policy_*` and `algorithm_*` test split, the statutory case catalogue, and the evidence gate (§7).
13. Amend ADR-0001 (§5.1, §5.2, §5.6) and ADR-0005 (§6); note the two-id consequence on ADR-0003 and ADR-0004; add ADR-0007 and ADR-0008.

Out of scope for this work: PostgreSQL, SQLx, Axum, React, Tauri, `PayrollRun` and finalization persistence, payslip UI, ETX, SSC filing, tax-period reporting, the remuneration-classification seam, calculating any of the four deduction kinds in §3.5 or their N$150,000 cap, using prior-employer figures in a calculation (SC-OPEN-4), concurrent employments, and non-monthly pay frequencies.

---

## 10. North star

> A payroll calculation is not compliant because the code is deterministic, well typed and well tested. It is compliant only when its classifications, statutory values, effective dates, calculation method and expected results are traceable to the Namibian rule or accepted payroll practice they claim to implement.

> Salt's agents may implement the rule. They may not decide what the rule is — and where no rule exists, they must say so out loud rather than dress a choice up as law.
