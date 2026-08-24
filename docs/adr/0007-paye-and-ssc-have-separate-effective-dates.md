# PAYE and SSC resolve on separate effective-date axes

`PayrollRules` no longer has one effective period covering everything. The PAYE band table and the social security rules each carry their own effective dates and their own identity — a `PayeTableId` and an `SscRulesId` — and `ruleset_for` resolves them independently, then freezes both into the single `PayrollRules` value `calculate()` receives. The calculator itself is unchanged.

We rejected a single combined identity, whether one `RulesetId` per pair or a composite name like `namibia-paye-2024-03+ssc-2026-09`. Namibia's PAYE table has not moved since 1 March 2024 while the SSC ceiling moves four more times before 2029. A single axis forced a new "PAYE version" every time only the ceiling changed, which is a fictitious version: it claims a PAYE rule changed when none did, and it makes the question "why is this table this?" unanswerable, because the id names two unrelated instruments at once. A stored calculation now records both ids, and each traces to exactly one source.

Each table also carries two dates rather than one. `legal_effective_from` is what the instrument says; `payroll_effective_from` is what payroll actually applies. Government Notice 236 states 1 March 2026 for the N$12,500 ceiling but was gazetted on 15 July 2026, and the SSC confirmed implementation from the September 2026 payroll with no retrospective adjustment. That gap is a calculation input, not a footnote, so it lives in the type. Source URLs and prose stay out of the pure value, in `docs/conformance/`.

## Consequences

- A ruleset is no longer named by one string. Anything that displayed a `RulesetId` shows two.
- `ruleset_for` gains a resolution step per axis, and a period whose two tables disagree about being in force is not representable — resolution either finds both or fails.
- Adding a third axis later (remuneration classification, once that seam exists) follows the same shape rather than needing a redesign.
- ADR-0003 still holds: these are typed Rust values shipped with the release, not database rows.
- ADR-0004 still holds: because code can change, neither id alone is a durable historical reference; a `FinalizedPayroll` stores the resolved values.
