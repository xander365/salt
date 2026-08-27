# Finalization is whole-run, recomputed, and compared to what was approved

A `PayrollRun` moves through `Draft → Calculated → Finalized`. There is **no `Reviewed` state**. Finalization is atomic across every included Employment, and it recalculates from current facts and current rules, refusing unless the result equals the stored `WorkingCalculation`. We rejected a `Reviewed` state, rejected per-Employment initial finalization, and rejected freezing the stored working calculation without recomputing it.

**`Reviewed` was ceremony.** Salt's users are Namibian SMEs where one bookkeeper does the payroll; there is no second person to review, and no separation of duties to enforce. A state that establishes no invariant still costs the machinery to invalidate it — every recalculation, every master-data change, every rules deployment has to know how to un-review a run. Finalizing is already a deliberate, audited, irreversible act. That is the approval. `CONTEXT.md` previously named four states and now names three.

**Initial finalization is whole-run.** One bad Employment blocking the run is the intent, not a cost: the employer fixes the data rather than shipping half a payroll. Per-Employment finalization would make "the run is finalized" mean nothing, and would let year-to-date be partially official for a period — a state nobody can reason about. ADR-0002 makes *corrections* per Employment, and that stays true; correcting a period is a different act from first paying it.

**Recomputing replaces every staleness mechanism.** The danger is ordinary: a user looks at a number, someone edits a salary or an opening balance, the user presses Finalize, and a figure nobody ever saw becomes permanent history. Recomputing under current facts and requiring equality with the approved calculation establishes the property directly — *the calculation that becomes history is exactly the one the user approved, under facts that are still current.* On mismatch, Salt refuses, names what changed, and requires a fresh calculation the user looks at again.

That is why there is no fingerprint, no revision counter, and no dependency-version tracking on `WorkingCalculation`. Those exist to detect staleness; recomputing *is* the detection, and it cannot miss a dependency nobody thought to fingerprint.

**Amended: the comparison is on all three frozen values, not the calculation alone.** Finalization reassembles the `PayrollInput` from current facts, re-resolves the `PayrollRules` via `ruleset_for(period.end)`, and recomputes the `PayrollCalculation` — and requires all three to equal what the `WorkingCalculation` stores.

Comparing only the output is not enough, and the failure is specific. A `FinalizedPayroll` freezes the input and the rules as well as the result (ADR-0004), and all three are what a future reader is shown. A PAYE table corrected in a band this employee never reaches, or a `CompensationTerms` `effective_from` fixed without touching `BasicPay`, produces a monetary output identical to the cent. Under an output-only comparison Salt would write a `payroll_rules_json` or a `payroll_input_json` into permanent history that nobody approved and no screen ever showed: the result correct, the explanation a fabrication, and INV-005 broken on a row that looks perfect.

It costs nothing. `PayrollInput`, `PayrollRules` and `PayrollCalculation` each already derive `PartialEq + Eq`. It is three `==`, not one.

## Consequences

- `Calculated` means **every** member has a current successful calculation. A partially calculated run is still `Draft`.
- `WorkingCalculation` needs no revision or fingerprint column, and recalculation may overwrite input, rules and result freely.
- Finalization is the only place the calculator runs against a transaction, and it runs once per member inside it.
- A run whose recompute disagrees is not an error to route around: it is the mechanism working. The refusal must name **which of the three** differed and what changed inside it, or users will learn to distrust it.
- A rule or input change that coincidentally produces the same monetary output still invalidates the approved working calculation. That is intended, not an over-strictness to relax.
