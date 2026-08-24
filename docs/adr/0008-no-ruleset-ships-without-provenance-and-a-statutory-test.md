# No ruleset ships without provenance and a statutory golden test

Every PAYE table and every social security ruleset in the shipped catalogue must have a provenance document in `docs/conformance/` naming its Tier A source, and at least one statutory golden case asserting a literal expected value from that source. A verification test walks every shipped id and **fails the test suite** — `cargo test`, and CI with it — if either is missing. We rejected relying on code review to catch it.

The gate reads an explicit **evidence catalogue**, not the source tree. An entry names a `rule_id`, a `provenance_document`, and the `statutory_case_ids` that prove it; the cases are themselves explicit data (`SC-PAYE-003`, `SC-SSC-003`, …) that the statutory tests iterate. We rejected scanning source files for test names beginning `statutory_`: that treats the test runner as a compliance registry, breaks under an ordinary rename, and can be satisfied by a name rather than by evidence. Test-name prefixes stay — as human semantics, for readers, not as the mechanism.

Salt already shipped synthetic PAYE brackets — `0 / 120k / 240k / 480k` — as the production Namibian table. They passed review, they passed every test, and the tests were green precisely because the same synthetic numbers were on both sides. Nothing in the codebase could tell a rule that had been checked against Namibia from one that had merely been typed in. This gate is that missing distinction, made mechanical.

A Salt-policy case cannot satisfy the gate. Those assert Salt's chosen per-period arithmetic, rounding, and part-month treatment, none of which is traceable to a published rule — a suite could be entirely green on policy tests while shipping invented statutory values, which is the exact failure again. The statutory case catalogue holds statutory cases only, so there is structurally nothing else for an evidence entry to reference.

## Consequences

- Adding a ruleset means adding a provenance document, a statutory case, and an evidence entry in the same change. This is deliberate friction on the most dangerous kind of edit.
- The verification suite proves five things per shipped id: an evidence entry exists; its provenance document exists on disk; it names at least one case id; every id it names exists in the case catalogue; and no non-statutory case can be referenced.
- Reading `docs/conformance/` through `CARGO_MANIFEST_DIR` is fine. INV-009 forbids I/O in *calculation*; it says nothing about test-time verification, and the production dependency graph stays I/O-free.
- Synthetic fixtures must stay unreachable through the public production resolvers, or the gate can be satisfied by the thing it exists to catch.
- Where no Tier A source exists at all, the answer is a `NEEDS ... CONFIRMATION` stamp in `docs/domain/statutory-conformance.md` and a `salt_policy_*` test — or, where Salt cannot even state what the rule would be, a refusal. Never a statutory claim.
