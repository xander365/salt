# Finalization freezes input, output, rule values, and Salt version

A `FinalizedPayroll` stores the complete `PayrollInput`, the complete `PayrollCalculation`, the resolved `PayrollRules` values (bands, rates, ceilings, rounding), the `RulesetId`, and the Salt version that produced it. We rejected storing references and rebuilding the input on demand.

Rebuilding means future code or future master data can silently change what history says, which breaks INV-005. Storing the rule *values* and not just the id matters because rules are code (ADR-0003): a later bug fix would otherwise change the meaning of every historical result that names that ruleset.

**Amended by ADR-0007.** A `FinalizedPayroll` stores **two** ids, not one — the `PayeTableId` and the `SscRulesId` that were resolved for the period — alongside the same complete resolved values. The reasoning is unchanged; there are simply two instruments to name instead of one.

**Amended by ADR-0013.** The frozen snapshot is the **sole** explainer of a historical payroll. Master data — `CompensationTerms`, the `UnsupportedDeductionStatus` declaration — stays correctable forever under an audited reason, precisely *because* nothing in this ADR ever depended on it. The snapshot froze those values, never a reference to those rows, so a later correction cannot reach a finalized figure. What this ADR forbids is history changing *silently*; a correction that is named, reasoned, attributed, and accompanied by the list of live finalized periods it now diverges from is the opposite of silent. Freezing the master rows as well would have been belt-and-braces buying nothing this ADR does not already buy, at the cost of the ability to correct a captured-wrong fact — which is exactly what a replacement calculation needs.

Reproducibility here means **explainable, not re-runnable**. Salt shows the stored input, rules, and output. It does not keep old calculator versions alive to recompute historical payroll bit-for-bit — that would mean never deleting a code path. The recorded Salt version serves the real need behind bit-for-bit: if a calculation bug is found, it identifies exactly which payslips were affected.
