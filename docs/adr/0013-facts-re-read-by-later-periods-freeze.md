# Facts later periods re-read freeze; facts consumed once do not

Salt holds four kinds of employment fact that reach a `PayrollInput`. Two of them — the `OpeningBalance` and the `PriorEmployment` declaration — become immutable at the Employment's first finalization in a TaxYear. Two of them — `CompensationTerms` and the `UnsupportedDeductionStatus` declaration — stay correctable forever, under an audited reason. We rejected freezing all four, which an earlier round of design had chosen.

The rule that separates them:

> A fact that **later periods re-read** freezes at finalization.
> A fact that is **consumed once and frozen into the snapshot** does not.

`OpeningBalance` and `PriorEmployment` are re-read into *every* later period's `YearToDateContext` (ADR-0001). Editing either after a finalization silently re-prices every future period's PAYE while the frozen snapshots still show the old figure — the exact "history quietly changed" failure ADR-0004 exists to stop. `CompensationTerms` and `UnsupportedDeductionStatus` are read once, for one period, and frozen into that period's `PayrollInput`. Nothing later reads them again, so editing them changes nothing that has already happened.

**Freezing all four looked safer and was not.** The case that breaks it: March, April and May reference one `CompensationTerms` row; all three are finalized; in June, March's underlying salary fact is found wrong. ADR-0002 lets March be reversed without cascading, but April and May keep the row live and therefore frozen — so the March replacement has no corrected fact to calculate from. And if only March was wrong, the truth is that the terms *differed* in March, so the honest repair is to split the row: March at the correct amount, the existing row starting 1-April. April and May keep the identical `BasicPay` they always had. That repair moves a frozen `effective_from`, so the freeze forbids the one edit that states the truth.

The guard the freeze was reaching for is real — a user must never be shown a payslip and an employment history screen that silently disagree. But the freeze was the wrong instrument, because ADR-0004 already settled that the **frozen snapshot**, not master data, explains a historical payroll. Freezing the master row as well bought nothing ADR-0004 did not already buy, and cost the ability to correct a captured-wrong fact. The guard is met instead by making the difference *visible*: every correction is an audited entry with a mandatory reason, before and after values, and the list of live finalized periods it now diverges from. A named, reasoned, attributed difference is the opposite of a silent one.

## Consequences

- Decision 26 of `docs/domain/payroll-run-persistence.md` is withdrawn. A `CompensationTerms` row referenced by live finalized payroll is editable.
- No finalized figure can move as a result. Year-to-date sums frozen numeric columns (ADR-0012), never master data, so only a reversal plus a replacement changes a cent.
- A correction run assembles its `PayrollInput` from current master data by the ordinary path. That is the legitimate source ADR-0002's replacement always needed and never had.
- The `OpeningBalance` freeze is per **Employment** and TaxYear, not per Employer. An Employment onboarded in June can still be given a boundary after other Employments finalized March.
- A new fact added later must be classified by this rule before it is persisted. "Is it re-read, or consumed once?" is the question, and the answer decides mutability.
