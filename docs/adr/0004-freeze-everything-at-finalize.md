# Finalization freezes input, output, rule values, and Salt version

A `FinalizedPayroll` stores the complete `PayrollInput`, the complete `PayrollCalculation`, the resolved `PayrollRules` values (bands, rates, ceilings, rounding), the `RulesetId`, and the Salt version that produced it. We rejected storing references and rebuilding the input on demand.

Rebuilding means future code or future master data can silently change what history says, which breaks INV-005. Storing the rule *values* and not just the `RulesetId` matters because rules are code (ADR-0003): a later bug fix would otherwise change the meaning of every historical result that names that ruleset.

Reproducibility here means **explainable, not re-runnable**. Salt shows the stored input, rules, and output. It does not keep old calculator versions alive to recompute historical payroll bit-for-bit — that would mean never deleting a code path. The recorded Salt version serves the real need behind bit-for-bit: if a calculation bug is found, it identifies exactly which payslips were affected.
