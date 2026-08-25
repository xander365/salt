# PAYE and SSC resolve on separate effective-date axes

`PayrollRules` no longer has one effective period covering everything. The PAYE band table and the social security rules each carry their own effective dates and their own identity — a `PayeTableId` and an `SscRulesId` — and `ruleset_for` resolves them independently, then freezes both into the single `PayrollRules` value `calculate()` receives. The calculator's own signature is unchanged, borrowed arguments included:

```text
paye_table_for(period_end) -> Result<&'static PayeTable,  PayrollError>
                        \
                         +--> ruleset_for(period_end) -> Result<PayrollRules, PayrollError>
                        /
ssc_rules_for(period_end) -> Result<&'static SscRuleset, PayrollError>

calculate(input: &PayrollInput, rules: &PayrollRules) -> Result<PayrollCalculation, PayrollError>
```

**`ruleset_for` is the one seam whose return type deliberately moved.** It used to hand back a `&'static PayrollRules` borrowed from a single static catalogue. Once the two halves resolve independently there is no single static value left to borrow, so it returns an **owned** composed `PayrollRules`. Each half still borrows from its own static catalogue; only the composition is owned. Callers that held a reference now hold a value and pass `&rules` into `calculate` — which is already what `calculate` wants.

We rejected a single combined identity, whether one `RulesetId` per pair or a composite name like `namibia-paye-2024-03+ssc-2026-09`. Namibia's PAYE table has not moved since 1 March 2024 while the SSC ceiling moves four more times before 2029. A single axis forced a new "PAYE version" every time only the ceiling changed, which is a fictitious version: it claims a PAYE rule changed when none did, and it makes the question "why is this table this?" unanswerable, because the id names two unrelated instruments at once. A stored calculation now records both ids, and each traces to exactly one source.

Each table also carries two dates rather than one. `legal_effective_from` is what the instrument says; `payroll_effective_from` is what payroll actually applies. Government Notice 236 states 1 March 2026 for the N$12,500 ceiling but was gazetted on 15 July 2026, and the SSC confirmed implementation from the September 2026 payroll with no retrospective adjustment. That gap is a calculation input, not a footnote, so it lives in the type. Source URLs and prose stay out of the pure value, in `docs/conformance/`.

## Consequences

- A ruleset is no longer named by one string. Anything that displayed a `RulesetId` shows two. `PayrollCalculation` records a `PayeTableId` and an `SscRulesId` side by side.
- `PayrollRules` carries **no identity and no effective period of its own** — only the resolved pair plus the rounding rule. An id or a period that changed whenever one half moved would recreate the combined axis this decision removes. Its constructor is consequently infallible: every invariant belongs to one of the three parts and has already been enforced by whoever built it, and a cross-half invariant here would be a combined axis by another name.
- `calculate` re-checks the period end date against **each half's own** payroll applicability, not a combined one, so a caller that bypasses `ruleset_for` cannot use a table that was not in force. The two checks are separate refusals — `PayeTableDoesNotCoverPeriod` and `SscRulesetDoesNotCoverPeriod` — for the same reason the resolution errors are separate.
- The two dates record a *deferral* and only a deferral. `payroll_effective_from` may fall after `legal_effective_from`, never before — payroll applying a rule earlier than the instrument grants it legal effect would withhold under a rule that did not yet exist. `PayeTable::new` and `SscRuleset::new` refuse that pair outright, so the direction of the gap is a type-level fact rather than a convention.
- `ruleset_for` gains a resolution step per axis, and a period whose two tables disagree about being in force is not representable — resolution either finds both or fails.
- Adding a third axis later (remuneration classification, once that seam exists) follows the same shape rather than needing a redesign.
- ADR-0003 still holds: these are typed Rust values shipped with the release, not database rows.
- ADR-0004 still holds: because code can change, neither id alone is a durable historical reference; a `FinalizedPayroll` stores the resolved values.
