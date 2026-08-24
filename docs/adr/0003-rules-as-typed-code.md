# Statutory rules are typed Rust, not database rows

PAYE bands, SSC rates and ceilings, and the rounding policy live in Rust as effective-dated values identified by a `RulesetId`, shipped with the release. We rejected storing them as editable database rows seeded by migrations.

Statutory rules change rarely, are dangerous when wrong, and need tests and compiler-checked shapes far more than they need an admin form. An untested rule row is how payroll goes wrong quietly. Employer-specific settings — the `PaySchedule`, for instance — are ordinary mutable data and stay in PostgreSQL; this decision covers statutory rules only.

## Consequences

- Changing a statutory rule means a release, which is correct: it should be reviewed and tested.
- Because code can change, an id alone is not a durable historical reference — see ADR-0004.
- **Amended by ADR-0007.** There is no single `RulesetId`. PAYE bands and SSC rules are separately identified and separately effective-dated, as a `PayeTableId` and an `SscRulesId`. Everything above still holds; it now holds twice, once per axis. This decision is unchanged in substance: statutory rules remain typed Rust shipped with the release, never editable configuration.
