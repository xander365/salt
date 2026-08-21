# Pay periods follow an employer pay schedule, not the calendar month

Most Namibian SMEs run payroll on a 26th-to-25th cycle rather than the calendar month. A `PaySchedule` on the Employer carries a `period_end_day` of 1–28, or a "last day of month" setting; Salt generates `PayPeriod`s from it. A calendar-month employer is `period_end_day = last day`, not a separate code path. Days 29, 30, and 31 are not accepted as raw values so February cannot produce a broken period.

Because periods are not calendar months, they straddle boundaries. Two rules resolve every case, both keyed on the **period end date**: it selects the applicable `PayrollRules`, and it selects the `TaxYear`. A period of 26 Aug – 25 Sep uses September's ruleset in full — statutory ceilings are monthly amounts, not daily accruals, so they are never split pro-rata. A period of 26 Feb – 25 Mar falls entirely in the new tax year, which gives every employer exactly 12 periods per tax year — a property cumulative PAYE (ADR-0001) depends on.

## Consequences

- Proration uses actual calendar days in that `PayPeriod` as the denominator, since periods vary between 28 and 31 days.
- `CompensationTerms` must begin on a period start date; mid-period effective dates are rejected. Proration therefore only ever concerns joiners and leavers.
