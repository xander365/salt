# Overtime is hours at a fixed multiplier, priced by a derived hourly rate

An Operator types overtime on the payroll worksheet as **hours per multiplier** — twelve hours at 1.5, four at 2.0 — and Salt computes the money (D14). Salt never receives an overtime amount.

The alternative was accepting money: let the Operator type N$1,246.15 and classify it as overtime. That needs no divisor and no `OrdinaryHours`, and it is the wrong answer. It pushes the arithmetic back into a spreadsheet, which is the exact problem Salt exists to remove, and it makes the figure uncheckable — nobody reading the payslip can tell whether 1,246.15 was right, because the inputs it came from were never recorded.

## The multiplier is the classification

The multiplier set is closed at **1.5 and 2.0** (D19, D31). It is an `enum` in the domain crate, not a `Decimal` field and not a configuration value: a third factor is a code change and a deliberate decision, exactly as a third `Earning` kind is. A free-text label carries the human reason — "Sunday overtime" — and is never read by any arithmetic.

⚠️ Whether 1.5 and 2.0 are the correct and only statutory factors in Namibia is **`Q-OPEN-8`**. They come from the owner's practice, not from a verified statute. Nothing in the UI or these docs may present them as law.

## The divisor is Salt's own choice, and it is stamped

For a salaried Employment the hourly rate is derived:

```
DerivedHourlyRate = BasicPay x 12 / 52 / OrdinaryHours
```

**This is a `SaltPolicy`, stamped `SC-OPEN-6`, `NEEDS CONFIRMATION`.** No published Namibian rule prescribing a divisor was found. It must never be described as law, and no test asserting a figure that depends on it may be named `statutory_*` (ADR-0008) — those are `salt_policy_*`.

`OrdinaryHours` is recorded per Employment on `CompensationTerms` (issue #75) precisely so the assumption is visible and dated, rather than hidden in a constant such as 173.33. An overtime line on a period whose terms row records no `OrdinaryHours` is **refused**, not guessed at; a salary-only period on the same row still pays, because missing hours are unknown, not invalid.

`BasicPay` here is the **contractual** figure on the terms row, never the prorated one. A person's hourly rate does not fall because they joined mid-month. That choice is part of `SC-OPEN-6` and is stamped with it.

## Rounding order, and exactly one rounding

The rate is kept as an exact reduced fraction and **never rounded** — a
repeating rate cannot be represented honestly as a finite decimal. Hours and
multiplier are applied to that fraction. The result is rounded **once**, at
the line, through the single existing rounding-policy seam (`RoundingRule`,
`SC-OPEN-2`).

This mirrors the asymmetric seam PAYE already uses: derivation returns an exact unrounded value, and the Salt rounding policy turns it into money. Two lines at the same multiplier stay two lines and round independently — they are never summed and rounded once, because a payslip has to be able to show each line and each shown line has to add up.

## Overtime and the social security base — settled law, not a choice

The Social Security General Regulations define `basic wage` as remuneration for ordinary work and **exclude** overtime from it (`docs/domain/statutory-conformance.md` §3.3). An `Overtime` earning therefore feeds `GrossRemuneration` and `TaxableRemuneration` and **never** the social security base.

This is stated in the one exhaustive two-way table in `RemunerationBases::accumulate`, like every other kind. Adding the kind made the compiler refuse the build until its effect on all three bases was decided, which is the intended behaviour.

## No blended-rate rule for a within-period change

A salary or `OrdinaryHours` change dated inside a pay period is refused by the **existing** effective-date and coverage rules (INV-014, `CompensationTermsDoNotCoverPeriod`). Exactly one `CompensationTerms` row covers every day being paid, and it supplies both the pay and the hours for the whole period. No blended-rate rule is written, and none is needed.

## Consequences

- `Earning` gains an `Overtime` kind carrying the money **and** an `OvertimeTrace` — structured data, like the PAYE and social security traces, with no free-text formula strings. The `SaltPolicy` mark rides as a `SaltPolicyStamp` value, so the screen renders the sentence and the calculator emits no user-facing English.
- `EarningInstruction::Overtime` carries hours and a multiplier and **no amount**, so the input type cannot express a figure someone worked out elsewhere. `EarningInstruction` therefore no longer has a total `amount()`.
- `PayrollFigures` gains `overtime` as a figure of its own, never folded into `taxable_allowances`: the two feed the bases differently, and a shared row would misstate what was paid.
- A member with an overtime line and no recorded `OrdinaryHours` is reported as the `OrdinaryHoursNotRecorded` run blocker before Calculate is pressed, and refused by the calculator if it is.
- Hourly-paid Employments and non-monthly frequencies remain out of scope (`Q-OPEN-17`). Accepting overtime settles neither.
