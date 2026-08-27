# The stateful layer is one crate with no repository seam

Salt becomes a Cargo workspace: `crates/payroll/` stays pure, and a single new crate `crates/payroll-app/` holds the use cases, the SQLx code and the migrations together. We rejected a Cargo feature flag on the existing crate, and we rejected splitting use cases from SQL into two crates behind a trait.

`crates/payroll/src/lib.rs` states that the crate has no web, database, or async dependency, and that INV-009 (no I/O in calculation) is therefore structurally true rather than a convention. It is true today because nothing is there. A `sqlx` optional dependency behind a feature would put SQLx in the crate's dependency graph and demote the claim to something a reviewer has to police. A second crate keeps the absence structural.

The second rejection matters more, because it is the one that looks like good practice. Splitting `payroll-app` into use cases and storage requires a seam between them, and a seam with exactly one implementation behind it is a `Repository<T>` by another name — the abstraction the design explicitly refuses. Explicit SQLx transactions inside explicit use-case functions (`finalize_payroll_run`, `build_year_to_date_context`) are the whole pattern. The split becomes worth making when there are two real implementations, and not before.

## Consequences

- The dependency direction is one-way: `payroll-app → payroll`, typed values only. The calculator never calls a repository.
- `payroll-app` wraps `PayrollError` in its own error type rather than re-exporting it. "PostgreSQL unavailable" and "run already finalized" are different categories and never share an enum.
- **Moving the crate breaks the ADR-0008 evidence gate and must be fixed in the same change.** `ruleset.rs` resolves provenance documents as `CARGO_MANIFEST_DIR / <provenance_document>`; moving the crate into `crates/payroll/` moves that root. `docs/conformance/` moves to `crates/payroll/docs/conformance/` — the evidence belongs with the code it proves. Rewriting the path as `CARGO_MANIFEST_DIR/../..` would make the test depend on the crate's depth in the tree. The failure is loud, not silent, but it is real.
- One derivation moves *into* the pure crate rather than out of it: `PeriodsElapsed` gains a constructor taking the PayPeriod end date. It is pure date arithmetic, it belongs beside the warning that already documents the failure it prevents, and one correct derivation in the pure crate is why nothing in `payroll-app` needs to write `COUNT(finalized_payroll)`.
