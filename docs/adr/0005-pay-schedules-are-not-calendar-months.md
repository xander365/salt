# Pay periods follow an employer pay schedule, not the calendar month

Most Namibian SMEs run payroll on a cycle that does not match the calendar month. A `PaySchedule` on the Employer carries a `period_end_day` of 1–28, or a "last day of month" setting; Salt generates `PayPeriod`s from it. A calendar-month employer is `period_end_day = last day`, not a separate code path. Days 29, 30, and 31 are not accepted as raw values so February cannot produce a broken period.

The rule, stated by the product owner and authoritative:

> The pay period end day is configurable. A pay period belongs to the month its **end date** falls in.

26th-to-25th is only the common example; 18th-to-17th is equally valid, and the pay date typically falls near the end of the month the period closes in. Because periods straddle month boundaries, two rules resolve every case, both keyed on the **period end date**: it selects the applicable `PayrollRules`, and it selects the `TaxYear`. A period of 26 Aug – 25 Sep uses September's rules in full — statutory ceilings are monthly amounts, not daily accruals, so they are never split pro-rata. A period of 26 Feb – 25 Mar falls entirely in the new tax year, which gives every employer exactly 12 periods per tax year — a property cumulative PAYE (ADR-0001) depends on.

**There is no separate `TaxPeriod` in v1.** PAYE is remitted within 20 days after the month in which the tax was deducted, which is calendar-month reporting sitting on top of non-calendar pay periods. That is a reporting clash, not a calculation clash: no figure Salt calculates today is wrong because of it, and the calculator is the only thing in scope. Introducing a `TaxPeriod` now would add a concept with no calculation to perform. When statutory reporting is built, the reporting month comes from `PayrollRun` and the pay date, and nothing stored before then needs revisiting.

## Consequences

- Proration uses actual calendar days in that `PayPeriod` as the denominator, since periods vary between 28 and 31 days.
- `CompensationTerms` must begin on a period start date; mid-period effective dates are rejected. Proration therefore only ever concerns joiners and leavers.
- The "monthly statutory amounts are never split pro-rata" rule extends to the social security floor and ceiling: a part-month joiner's prorated `BasicPay` is clamped to the full monthly N$500 and the full monthly ceiling. Whether the SSC agrees is unresolved (SC-OPEN-3), so that behaviour is a `salt_policy_*` test, not a `statutory_*` one.
- Since ADR-0007 gave PAYE and SSC separate effective-date axes, the period end date now selects *each* of them independently, through `paye_table_for` and `ssc_rules_for`. The keying rule is unchanged; it simply applies twice.
- **Selecting a ruleset by period end date is Salt policy, not a statutory claim.** The rule above is the product owner's, and no published Namibian source prescribes which ruleset a period straddling a rate change must use. It is defensible — statutory ceilings are monthly amounts, and splitting one pro-rata would invent an amount no instrument states — but it is Salt's decision. A test asserting that a 26 August – 25 September 2026 period uses the September SSC ceiling for its whole length is therefore named `salt_policy_*`, never `statutory_*`, and it may never be cited as conformance evidence under ADR-0008. What *is* statutory is the ceiling value itself and the date its instrument takes effect; how Salt maps a pay period onto that date is not.
