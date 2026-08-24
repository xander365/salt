# Rule provenance

One file per shipped rule table. Every `PayeTableId` and every `SscRulesId` in the catalogue has one, and ADR-0008 fails the build if it does not.

Each file answers one question in one hop:

> Why is this value this number, and why does Salt start using it on that date?

The pure `PayrollRules` value carries the numbers and the two dates. It does not carry URLs. These files do.

## File shape

```text
# <TableId>

**Kind:** PAYE table | SSC rules
**legal_effective_from:**    what the instrument says
**payroll_effective_from:**  what payroll actually applies
**Statutory cases:**         the SC-PAYE-* / SC-SSC-* ids that prove it

## Values
## Sources
## Notes
```

The case ids are not decoration. A `ConformanceEvidence` entry in the code names this document and those ids, and the verification suite fails if the document is missing, if no id is named, or if a named id does not exist in the statutory case catalogue (ADR-0008). The cases themselves are listed in `docs/domain/statutory-conformance.md` §7.

`legal_effective_from` and `payroll_effective_from` are usually the same date. When they differ — a gazette published after its own stated effective date, a regulator deferring implementation — the Notes section must say why (ADR-0007).

Open questions that no source answers do **not** live here. They live in `docs/domain/statutory-conformance.md` §4 with a `NEEDS ... CONFIRMATION` stamp.
