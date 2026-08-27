# Corrections are one Employment to a run, in an explicit chain

A `PayrollRun` with `kind = Correction` holds **exactly one** Employment, added explicitly, and carries a mandatory `correction_reason`. Nothing is auto-proposed into it. Its membership names `replaces_finalized_payroll_id`, which is UNIQUE. We rejected auto-proposing every Employment that overlapped the historical period, and rejected letting one Correction run hold several Employments.

**Ordinary and Correction membership are opposites, and that is the point.** An Ordinary run auto-proposes every Employment overlapping the PayPeriod, because silent omission is the dangerous failure — an employee nobody noticed was not paid — so removal is the deliberate, reasoned, logged act. A Correction run inverts both halves: nothing is proposed, and inclusion is the deliberate act. Sweeping in every Employment that happened to be employed in an old period is how a one-employee fix becomes a nine-employee re-issue of payslips people already have.

**One Employment rather than several**, because ADR-0002 already made corrections per Employment. A multi-member Correction run would re-import the whole-run atomicity of ADR-0010 into an act deliberately kept per Employment, and would let one refusal block four unrelated fixes. With one member, "all included Employments finalize, or none" stays literally true with no special case. The cost — an employer correcting five employees for one bad rate creates five runs — is honest, because it is five reversals and five replacements either way.

**Lineage is a chain, not a tree.** `replaces_finalized_payroll_id` is set at insert, never updated, and UNIQUE, so a given `FinalizedPayroll` is replaced at most once:

```text
F1  →  reversed  →  F2 (replaces F1)  →  reversed  →  F3 (replaces F2)
```

The target must be reversed, not live, and must match the Employment, Employer and period end. Inference from `(employment_id, period_end)` ordered by `finalized_at` is an argument that collapses the second time a period is corrected; a foreign key answers "what did this replace" outright.

**The column is null in exactly two cases, and they are why it is not a simple biconditional.** An Employment may have been removed with a reason from the finalized Ordinary run for that period and the removal turns out to have been wrong; or it may never have been a member at all, having been created later with a backdated `start_date`. In both there is no `FinalizedPayroll` to reverse, and only one Ordinary run per Employer and PayPeriod is permitted, so refusing would mean that employee can never be paid for that period. Paying the money in a later ordinary period instead would attribute it to the wrong PayPeriod and scale their PAYE bands against the wrong `period_number` (ADR-0001). The mandatory `correction_reason` carries what happened.

## Consequences

- PostgreSQL enforces UNIQUE, the foreign key, the matching employment and period, `correction_reason` non-empty when `kind = Correction`, and at most one membership row on a Correction run. Rust enforces "lineage is set exactly when a reversed predecessor exists", which is a domain question SQL cannot ask.
- A Correction run checks its own period only — neither earlier nor later. Earlier periods were checked when the original finalized and cannot become unresolved; checking later ones would contradict the settled rule that a period may be corrected after later periods are finalized.
- A Correction run's earnings are pre-populated from the reversed `FinalizedPayroll`'s frozen input and then edited. Where that snapshot's `snapshot_schema_version` is not one the running Salt deserializes, the run starts empty and says so — ADR-0004 promises history is explainable, never that every old snapshot deserializes forever.
