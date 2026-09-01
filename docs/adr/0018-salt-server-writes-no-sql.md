# `salt-server` writes no SQL; `payroll-app` owns the whole schema, identity included

The new delivery crate `crates/salt-server` (Axum, pool ownership, authentication, HTTP DTOs, error mapping) does not depend on `sqlx` and contains no SQL. The `operator`, `employer_membership` and `session` tables and their queries live in `payroll-app`, beside payroll's own, even though identity is not payroll. We rejected a third `salt-auth` crate and rejected putting identity SQL in `salt-server`.

The blunt reason is mechanical: `sqlx` keeps one `_sqlx_migrations` table per database, so a second migration directory against the same database fights the first. Both rejected options need one.

The better reason is the rule it buys. "`salt-server` writes no SQL" is checkable by a reader in one glance at `Cargo.toml`, and it is a stronger guarantee than "identity lives elsewhere": it means no HTTP handler can reach past a use case into a table, which is the failure mode the layering exists to prevent. A `salt-auth` crate would also buy a boundary with exactly one consumer — the seam ADR-0009 already refused to build for storage.

A later review found the rule stated but not implementable: every use case took
`pool: &PgPool`, so naming a pool would itself require the forbidden dependency.
`payroll-app` therefore exports `SaltDatabase` — an opaque handle over a `PgPool`
with no accessor, no `Deref` and no `Into` — plus `SaltDatabase::connect(&DatabaseConfig)`,
which builds the pool, installs the `after_connect` hook that issues
`SET ROLE payroll_app`, and verifies the applied migration version. Every use-case
signature takes `&SaltDatabase`. `DatabaseConfig` is plain values the server fills
from the environment, with no `sqlx` type in it.

That is a pool type and a constructor, not a `Database` trait: ADR-0009's refusal of
a seam with one implementation behind it stands. The opacity is what makes this ADR
structural rather than a convention — a `SaltDatabase` cannot be unwrapped, and the
`from_pool` escape hatch needs a `PgPool` value, which a crate without `sqlx` cannot
produce.

## Consequences

- `payroll-app` is best read as "the crate that owns the schema and the use cases over it", not "the payroll crate". Identity modules sit beside payroll ones and share the migration sequence.
- `salt-server`'s dependency list is the enforcement. Adding `sqlx` to it is the thing to reject in review.
- `SET ROLE payroll_app` is a property of the pool, not of any call site, so no use case can forget it and no future pool refill can escape it.
- `salt-server` is a library plus a thin binary: the library exposes the router so HTTP tests build the real thing in-process, and the binary loads config, creates the pool and listens.
