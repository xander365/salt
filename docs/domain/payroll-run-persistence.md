# Salt — Payroll Run Persistence, Finalization, Reversal and YTD Reconstruction

**Status:** Settled by `/grill-with-docs`, 2026-08-27.
**Purpose:** The stateful payroll model surrounding the pure calculator.
**Next step:** `/to-spec` — one implementation spec for *persist and finalize an
ordinary monthly payroll run, then reconstruct year-to-date from that immutable
history for the following run.*

This document records **what was decided and why**. It is not an implementation
spec: no column types beyond those that carry a decision, no migration order, no
function signatures.

---

## 1. The goal

Salt already has a pure calculator:

```text
PayrollInput + PayrollRules → calculate → PayrollCalculation
```

The stateful layer exists to make one sentence true:

> A finalized payroll fact is durable, explainable, immutable, and safe to use
> as input to the next payroll calculation.

Storing data is not the milestone. The milestone is that **March's finalized
history correctly feeds April's calculation**, and still does after March is
reversed and replaced.

---

## 2. Inherited constraints

These come from existing ADRs and were not reopened.

| Constraint | Source |
| --- | --- |
| Finalized payroll is immutable | INV-004, ADR-0002 |
| Finalization freezes values, not references | ADR-0004 |
| Corrections are reversal plus optional replacement, per Employment | ADR-0002 |
| Corrections never cascade into later periods | ADR-0002, ADR-0001 |
| Year-to-date is summed from live records, never stored as a running total | INV-013, ADR-0001 |
| `OpeningBalance` and `PriorEmployment` are different facts and never merge | ADR-0001, CONTEXT.md |
| Immutable rows plus an append-only action log; no event sourcing | ADR-0006 |
| No deletion of payroll history | ADR-0006 |
| Statutory rules are typed Rust, not database rows | ADR-0003, ADR-0007 |
| The period **end date** selects rules and TaxYear; 12 periods per TaxYear | ADR-0005 |

Four new ADRs record the rationale this grill produced:

| ADR | Decision |
| --- | --- |
| ADR-0009 | The stateful layer is one crate with no repository seam |
| ADR-0010 | Finalization is whole-run, recomputed, and compared to what was approved |
| ADR-0011 | Liveness is a separate table, so finalized payroll is never updatable |
| ADR-0012 | Year-to-date sums real columns, never the frozen snapshots |

---

## 3. Architecture

### 3.1 Workspace shape

The repository becomes a Cargo workspace:

```text
Cargo.toml                 [workspace]
crates/payroll/            the existing pure crate, moved from src/
crates/payroll-app/        use cases, SQLx, migrations
```

`crates/payroll` keeps the promise its `src/lib.rs` already states: no web,
database, or async dependency. That promise is currently true *because nothing
is there*, and a Cargo feature flag would demote a structural fact to a
convention. A second crate keeps it structural.

`payroll-app` is **one** crate holding use cases, SQL and migrations together.
Splitting use cases from SQL needs a seam between them, and a seam with exactly
one implementation behind it is the generic repository this design rejects
(§12). Split it the day there is a second implementation.

### 3.2 Migration hazard: ADR-0008's evidence gate

`ruleset.rs` resolves provenance documents as
`CARGO_MANIFEST_DIR / <provenance_document>` (currently `src/ruleset.rs:579`).
Moving the crate into `crates/payroll/` moves `CARGO_MANIFEST_DIR` and **breaks
the ADR-0008 verification gate**. It breaks loudly — the suite fails — but it
must be handled in the same change.

Resolution: move `docs/conformance/` to `crates/payroll/docs/conformance/`. The
evidence belongs with the code it proves, and the gate is the pure crate's own
gate. Rewriting the path as `CARGO_MANIFEST_DIR/../..` would make the test
depend on the crate's depth in the tree.

### 3.3 The dependency direction

```text
payroll-app  →  payroll        (typed values, one direction only)
```

```text
application loads facts
  → constructs PayrollInput
  → resolves PayrollRules via ruleset_for(period.end)
  → calls pure calculate()
  → persists result
```

Never `calculator calls repository`.

`payroll-app` wraps `PayrollError` in its own error type rather than
re-exporting it. Infrastructure failure ("PostgreSQL unavailable") is a
different category from domain state ("run already finalized") and the two never
share an enum.

### 3.4 One derivation moves into the pure crate

`PeriodsElapsed` gains a constructor deriving it from the PayPeriod end date —
the period's position in the TaxYear. It is pure date arithmetic and belongs
next to the existing warning at `year_to_date.rs:9`. One correct derivation in
the pure crate is why nobody in `payroll-app` ever needs to write
`COUNT(finalized_payroll)`.

---

## 4. Persisted concepts

### 4.1 Identifiers

`EmploymentId`, `EmployerId` and `PersonId` are opaque `String`-backed types by
design (`employment.rs:12`) and become `TEXT` columns holding a UUIDv7 string.
The calculator must not care what shape an id is, so making the pure crate
UUID-backed would drag storage into it.

Ids owned by `payroll-app` — `PayrollRunId`, `FinalizedPayrollId`,
`ReversalId`, `OpeningBalanceId` — are native `uuid`.

Every actor column (`created_by`, `finalized_by`, `reversed_by`) is plain
`TEXT` holding an operator identifier. There is no `User` table: authentication
is out of scope, and a `TEXT` column becomes a foreign key later with one
migration.

### 4.2 Employer

One `PaySchedule` per Employer, mutable — **but a change is refused once any
payroll is finalized in the current TaxYear**, so it can only take effect from
the next TaxYear.

ADR-0005 guarantees exactly 12 periods per TaxYear and cumulative PAYE
(ADR-0001) depends on that. Moving `period_end_day` mid-year breaks the
guarantee. A change can therefore only be legal at a tax-year boundary, which
makes a full effective-dated `PaySchedule` history a third effective-dating axis
built for something that changes once a decade. History is safe either way,
because `PayrollInput` freezes the schedule that was used.

### 4.3 Employment

`Employment` is **never physically deleted**. Ending an Employment is an
`end_date`. A mis-created Employment is marked void and never appears in run
membership. "Delete when nothing references it" is a racing check against a
moving target; a boolean is cheaper and leaves the mistake visible in the
action log.

### 4.4 CompensationTerms

Unique on `(employment_id, effective_from)`. `effective_from` only — no
`effective_until`; a row is in force until the next row's `effective_from`, and
INV-014 already pins every `effective_from` to a pay-period start date.

**A `CompensationTerms` row becomes immutable once a live `FinalizedPayroll`
references it.** Unreferenced and future-dated rows stay freely editable.

If referenced rows stayed editable, the frozen payslip and the employment
history screen would tell two different stories about the same month — and the
history screen is what a user reads to explain the payslip. A real error in an
already-paid period is a payroll correction (reverse and replace), never a quiet
master-data edit.

### 4.5 OpeningBalance

Unique on `(employment_id, tax_year)`. Same Employment, same Employer — never
another employer's figures, which are `PriorEmployment` and are refused
(SC-OPEN-4).

**An explicit row is required before any payroll in that TaxYear may finalize,
zeros included.** Salt auto-creates a zero row when an Employment is created.

This is INV-012 at the persistence layer. A missing row cannot be distinguished
from "nobody asked", which is exactly the ambiguity `PriorEmployment::Unknown`
exists to prevent inside the calculator. Absence of data is never read as
absence of the condition.

**The balance freezes on the first finalization in its TaxYear.** Freely
editable before; immutable after. Correcting it then requires reversing the
finalized payroll that froze it.

An editable balance after finalization would silently change every future
period's PAYE while the frozen snapshots still showed the old figure — the exact
"history quietly changed" failure ADR-0004 exists to stop.

`periods_elapsed` is **not** stored on `OpeningBalance`. It is derived from the
PayPeriod's position in the TaxYear (§8).

### 4.6 PayrollRun

```text
PayrollRun
- PayrollRunId
- EmployerId
- PayPeriod
- pay_date
- kind      Ordinary | Correction
- status    Draft | Calculated | Finalized
- created_at, created_by
```

`pay_date` lives here. `CONTEXT.md` already promised it, ADR-0005 expects it
when statutory reporting is built, and it is **never a calculation input** — it
does not reach `PayrollInput`.

**Uniqueness applies to `Ordinary` only:** one Ordinary run per Employer +
PayPeriod. Correction runs are unconstrained, because a period may legitimately
be corrected more than once.

`status` is a **stored column**, not derived. The finalization transaction takes
`SELECT … FOR UPDATE` on this row. A derived status offers nothing to lock,
which is precisely how two processes both conclude they may finalize.

### 4.7 PayrollRun lifecycle

```text
Draft  →  Calculated  →  Finalized
```

**There is no `Reviewed` state.** Finalizing is itself the deliberate approval,
and a one-bookkeeper SME has no second person to review. A ceremonial state that
enforces no invariant is worse than no state: it costs review-invalidation
machinery and buys a checkbox. `CONTEXT.md` has been updated.

- **Draft** — the run exists; membership may change; no calculation is claimed
  current.
- **Calculated** — **every** member has a current, successful working
  calculation. A partially calculated run is still `Draft`.
- **Finalized** — immutable history was created atomically; working state can no
  longer change; the run cannot finalize again.

### 4.8 Run membership

```text
PayrollRunEmployment
- PayrollRunId, EmploymentId
- removed_at, removed_by, removal_reason   (nullable, set together)
```

Creating a run writes a membership row for **every Employment overlapping the
PayPeriod**. A user may remove one before calculation, and **a removal requires
a non-empty reason**.

Silent omission is the dangerous failure — an employee nobody noticed was not
paid. Auto-inclusion makes omission a deliberate, reasoned, logged act, and the
reason is the thing the employer needs six months later.

### 4.9 Working calculation

```text
WorkingPayrollCalculation
- PayrollRunId, EmploymentId          UNIQUE together
- PayrollInput snapshot
- PayrollRules snapshot
- PayrollCalculation
- calculated_at, calculated_by
```

Recalculation overwrites the row. Input, rules and result are overwritten in one
statement — a row mixing one calculation's input with another's result is not
representable. This is working state (ADR-0006), and it needs no fingerprint and
no revision number, because finalization recomputes rather than trusting it
(§5.2).

---

## 5. Finalization

### 5.1 Whole-run atomic

**All included Employments finalize, or none.** One bad Employment blocking the
run is the intent, not a cost: the employer fixes the data instead of shipping
half a payroll. Per-Employment initial finalization would make "the run is
finalized" mean nothing and would let year-to-date be half-official.

ADR-0002 makes *corrections* per Employment. That is a different act, and it
stays per Employment.

### 5.2 Recompute and require equality

Finalization recalculates from **current facts and current rules**, and refuses
unless the result **equals** the stored working calculation.

```text
current facts + current rules → calculate → must equal working calculation → finalize
```

This establishes:

> The calculation that becomes history is exactly the one the user approved on
> screen, under facts that are still current.

Without it, a salary edit between "look at the number" and "press Finalize" puts
a number into history that nobody ever saw. Comparison is one `==`:
`PayrollCalculation` derives `PartialEq` (`calculation.rs:383`). A mismatch
refuses, names what changed, and demands a fresh calculation the user looks at
again.

This is why no fingerprint, revision or staleness-detection machinery is needed.
Recomputing *is* the staleness check.

### 5.3 The transaction

```text
BEGIN
 1. SELECT … FOR UPDATE the PayrollRun row
 2. verify status is Calculated and kind/period are as expected
 3. verify every member Employment has an OpeningBalance for the TaxYear
 4. verify the previous period is resolved for every member (§7)
 5. recompute every member and require equality with its working calculation
 6. INSERT the immutable FinalizedPayroll rows
 7. INSERT the live_finalized_payroll rows        ← the concurrency guard
 8. INSERT the action-log entries
 9. UPDATE payroll_run SET status = 'Finalized'
COMMIT
```

Any failure rolls back. There is no partial history, and no path writes a
`FinalizedPayroll` without its liveness row and its audit entry.

### 5.4 Concurrency

The invariant:

> For one Employment and one PayPeriod, at most one **live** finalized payroll
> exists, whatever two application processes believe.

Two mechanisms, in this order:

1. The `FOR UPDATE` lock on `payroll_run` serialises two finalizers of the same
   run.
2. The primary key on `live_finalized_payroll` (§6.2) makes a second live row
   for the same `(employment_id, period_end)` **unrepresentable**, regardless of
   which run, which process, or what the application believed.

`READ COMMITTED` is sufficient: correctness rests on the lock and the key, not
on isolation level. Neither mechanism depends on an application-side
`if status != Finalized`.

---

## 6. Reversal, replacement, and liveness

### 6.1 Reversal

```text
Reversal
- ReversalId
- FinalizedPayrollId          UNIQUE — one reversal per finalized payroll
- reversed_at, reversed_by
- reason                      mandatory, non-empty
```

The original `FinalizedPayroll` is untouched. Reversal is immutable.

**A reversal cannot be undone.** A mistaken reversal is corrected by finalizing
a replacement identical to the original. The live set already produces the right
arithmetic with no new record type, and the trail reads honestly: reversed, then
reinstated by a replacement. An "unreversal" would be a fourth history shape
built for one rare mistake.

### 6.2 Liveness is a table, not a column

```text
live_finalized_payroll
- (employment_id, period_end)   PRIMARY KEY
- finalized_payroll_id
```

Finalizing inserts. Reversal deletes. A replacement inserts again.

The alternative — a nullable `reversed_by` column on `finalized_payroll` with a
partial unique index — requires the application to hold `UPDATE` on the table
whose whole point is that it is never updated. With liveness held separately,
Salt can:

```sql
REVOKE UPDATE, DELETE ON finalized_payroll FROM <app role>;
```

INV-004 becomes a **database permission** rather than application discipline.
The one primary key blocks duplicate initial finalization *and* duplicate
replacement. The deleted liveness row is not history — the `Reversal` row is.

### 6.3 Replacement

A replacement lives in a **new `PayrollRun` with `kind = Correction`** for the
same PayPeriod. Reopening the original run would mutate a finalized run, which
§5.1 forbids, and a correction has its own pay date and its own audit trail
because it is genuinely its own event.

Lineage is **explicit**: the replacement carries a nullable
`replaces_finalized_payroll_id`, set at insert and never updated. §13's success
condition is what an auditor can inspect; a foreign key answers "what did this
replace" outright, where inference from `(employment_id, period_end)` ordered by
`finalized_at` is an argument that weakens the moment a period is corrected
twice.

Replacement is **optional**. A bare reversal is valid and arithmetically
complete (ADR-0002), because year-to-date sums only live records. The
consequence is explicit and accepted: a newly-built later year-to-date drops the
reversed period immediately.

### 6.4 Correcting a period when later periods are finalized

Allowed. Later periods are never rewritten; cumulative PAYE absorbs the
difference at the next calculation (ADR-0001, ADR-0002). Salt warns and names
the later finalized periods.

**One consequence stops being theoretical here.** ADR-0001 refuses when
recalculated liability falls below PAYE already withheld. Corrections that lower
earlier figures are how a real user reaches that state. It stays a refusal —
refunds are not modelled anywhere in this domain.

---

## 7. Ordering and sequencing

Payroll order is **`PayPeriod.end_date`**, never `finalized_at`. A
late-finalized March is still March, and a replacement finalized in October is
still the March figure for year-to-date purposes.

**Ordinary payroll finalizes in pay-period order.** April refuses until March is
resolved for that Employment. Cumulative PAYE is *wrong*, not merely
incomplete, when a period silently vanishes; refusing converts silent
under-withholding into a visible error.

**A period is resolved for an Employment when** it has a live `FinalizedPayroll`
for that period, **or** it was a removed member — carrying its mandatory reason
(§4.8) — of the finalized Ordinary run for that period.

There is deliberately **no separate `NoPayrollRecord` concept**. The reasoned
removal already records the fact, and two ways to say one thing is how the two
drift apart.

---

## 8. Year-to-date reconstruction

```text
prior_taxable_remuneration =
    OpeningBalance.prior_taxable_remuneration
  + SUM(live FinalizedPayroll.taxable_remuneration
        WHERE same Employment, same TaxYear, period_end < this period_end)

prior_paye =
    OpeningBalance.prior_paye
  + SUM(live FinalizedPayroll.paye  — same predicate)

periods_elapsed =
    the PayPeriod's position in its TaxYear     (§3.4)
```

**`periods_elapsed` is never `COUNT(finalized_payroll)`.** It is tax-year
position, and reading it as periods paid over-withholds from every mid-year
joiner — the failure `year_to_date.rs:9` documents at length.

`live` means: joined through `live_finalized_payroll`. A reversed period is
absent because its liveness row was deleted; a replacement is present because it
inserted a new one. Nothing in the year-to-date query mentions reversals.

**The sums read real numeric columns, never JSONB.** `finalized_payroll` stores
`taxable_remuneration` and `paye` as numeric columns beside the snapshots.

This is the sharp edge of §9.1: summing JSON paths would let a Rust field rename
break arithmetic across years of history. Columns keep year-to-date correct
independently of whether an old snapshot still deserializes. They agree with the
snapshot by construction — one `INSERT`, one value, and the row is never
updated.

---

## 9. FinalizedPayroll and frozen snapshots

```text
FinalizedPayroll
- FinalizedPayrollId
- PayrollRunId, EmploymentId, EmployerId
- PayPeriod (start, end), TaxYear
- replaces_finalized_payroll_id     nullable, never updated

- payroll_input_json                JSONB
- payroll_rules_json                JSONB
- payroll_calculation_json          JSONB

- taxable_remuneration              numeric   ← year-to-date reads these
- paye                              numeric

- PayeTableId, SscRulesId
- salt_version
- snapshot_schema_version           integer, initially 1

- finalized_at, finalized_by
```

Rows are **never updated and never deleted**, enforced by `REVOKE` (§6.2), not
by discipline. Audit metadata is as immutable as the figures.

Every domain type already derives `Serialize` and `Deserialize`, so JSONB
snapshots cost nothing today.

### 9.1 Explainable, not re-runnable

ADR-0004 already draws this line and it governs the snapshot design:

> Keep the raw historical data forever — **not** every old snapshot must
> deserialize into the newest Rust structs forever.

`snapshot_schema_version` ships from day one. `SaltVersion` alone is technically
recoverable, but the recovery is a maintained table mapping every Salt version
to a snapshot shape. An integer states it directly, and it is the field a future
reader branches on to render old history without constructing current domain
types.

**There are no in-place JSON migrations, ever.** This is forced, not chosen: the
application has no `UPDATE` grant on the table. A shape change means new rows
carry version 2 and readers branch on the version.

### 9.2 SaltVersion

Semver plus git SHA, one string: `0.1.0+g1a2b3c4`, captured in `build.rs`.

The SHA is what actually locates the code that produced a historical payroll;
the semver is what a human quotes in a support ticket. **A release build from a
dirty working tree must fail**, or the SHA is a lie.

---

## 10. Action log

```text
ActionLogEntry
- ActionLogEntryId
- EmployerId
- actor, occurred_at
- action_type            typed enum, never free text
- target_type, target_id
- context                JSONB, optional
```

Logged: `PayrollRunCreated`, `EmploymentRemovedFromRun` (with its reason),
`PayrollFinalized`, `FinalizedPayrollReversed`, `OpeningBalanceCreated`,
`OpeningBalanceChanged`, `PayScheduleChanged`, `EmploymentVoided`.

**Working recalculations are not logged.** The `working_calculation` row already
carries `calculated_at` and `calculated_by`, and only the last calculation can
become history. Logging eleven recalculations while a user fixes a typo buries
the entries an auditor needs.

Reads are not logged in v1.

Finalization and reversal write their log entries **in the same transaction** as
the fact they record. A committed finalization with no audit entry is not a
reachable state.

---

## 11. Where invariants live

**PostgreSQL** — anything structural or concurrency-sensitive:

```text
foreign keys, NOT NULL
UNIQUE (employment_id, tax_year)              one OpeningBalance
UNIQUE (payroll_run_id, employment_id)        one working calculation
UNIQUE (finalized_payroll_id) on reversal     one reversal
PK (employment_id, period_end) on liveness    one live payroll
one Ordinary run per Employer + PayPeriod
REVOKE UPDATE, DELETE on finalized_payroll
```

**Rust** — anything requiring domain reasoning:

```text
Employment overlaps the PayPeriod
applicable CompensationTerms
calculator support and refusals
lifecycle transitions
previous-period-resolved checks
recompute-and-compare
```

Not every domain rule is duplicated in SQL. No concurrency-safe structural
invariant is left to application memory.

---

## 12. Application seams

Explicit functions, not a framework:

```text
CreatePayrollRun            AddEmploymentToPayrollRun
RemoveEmploymentFromRun     CalculatePayrollRun
FinalizePayrollRun          ReverseFinalizedPayroll
GetPayrollRun               GetFinalizedPayroll
BuildYearToDateContext
```

No `Repository<T>`, no `UnitOfWork`, no `Specification<T>`. Explicit SQLx
transactions. Abstraction arrives when there are two real implementations.

---

## 13. The success conditions, answered

**An employer finalizes March. April builds year-to-date from it. Months later
March is found wrong, reversed and replaced. What exists?**

Rows: the original `FinalizedPayroll` (untouched), a `Reversal` naming it with a
mandatory reason, and a replacement `FinalizedPayroll` in a Correction run whose
`replaces_finalized_payroll_id` names the original. All three are immutable.
`live_finalized_payroll` for `(employment, 31-March)` pointed at the original,
was deleted by the reversal, and now points at the replacement. A newly-built
May year-to-date sees the replacement only — it joins through liveness and never
mentions reversals. April's own finalized snapshot is untouched; ADR-0001
absorbs the difference at the next calculation. An auditor inspects three
immutable rows, the frozen input, rules and result on each, and the action-log
entries that committed with them.

**Two users finalize the same run simultaneously.**

Both take `SELECT … FOR UPDATE` on the `payroll_run` row, so one waits. The
loser then finds `status = 'Finalized'` and refuses. Even if that check were
removed, the primary key on `live_finalized_payroll` makes the second insert
fail: duplicate live history is not representable, not merely guarded against.

**Master data and rule code change six months later. Why can March still be
explained?**

`FinalizedPayroll` froze the complete `PayrollInput`, the complete resolved
`PayrollRules` values, the complete `PayrollCalculation`, both rule ids, the
snapshot schema version and the Salt version (ADR-0004, ADR-0007). Nothing is
rebuilt from current employee records or current rule code. The
`CompensationTerms` row March used is itself frozen the moment a live finalized
payroll references it (§4.4), so the history screen cannot contradict the
payslip either.

---

## 14. Tests this design owes

The tracer bullet — the one that proves the architecture:

```text
GIVEN  Employer E, monthly PaySchedule, Employment A, CompensationTerms,
       an explicit zero OpeningBalance
WHEN   a March Ordinary run is created, A included, calculated, finalized
THEN   FinalizedPayroll freezes input, rules, result, both rule ids,
       snapshot schema version and Salt version
WHEN   an April run is created and calculated
THEN   April's YearToDateContext = OpeningBalance + live March FinalizedPayroll
AND    April's prior taxable remuneration and prior PAYE equal March's
       finalized outputs plus the OpeningBalance
WHEN   March is reversed and a replacement finalized
THEN   a newly-built later year-to-date sees the replacement, not the original
```

Transaction and integration tests:

1. concurrent double finalization cannot create duplicate live history;
2. failed finalization leaves no partial `FinalizedPayroll` rows;
3. finalization and its audit entry commit together;
4. reversal and its audit entry commit together;
5. one `FinalizedPayroll` cannot be reversed twice;
6. reversed history is excluded from year-to-date;
7. replacement history is included;
8. a working-calculation overwrite cannot modify finalized history;
9. later Employment/CompensationTerms changes cannot alter frozen history;
10. year-to-date orders by pay period, not finalization timestamp;
11. `periods_elapsed` is never derived by counting rows;
12. finalizing April refuses while March is unresolved;
13. a `CompensationTerms` row referenced by live finalized payroll refuses edits;
14. finalization refuses when a recompute disagrees with the working calculation;
15. an `UPDATE` on `finalized_payroll` is refused by the database role itself.

---

## 15. Still open

| Id | Question |
| --- | --- |
| OPEN-P1 | Actors are bare `TEXT`. When authentication arrives, `actor` becomes a foreign key — no user model is designed here. |
| OPEN-P2 | ADR-0001's below-withheld refusal becomes user-reachable through corrections (§6.4). What a user is told, and what they do next, is a product question this design does not answer. |
| OPEN-P3 | Migration tooling (`sqlx migrate` or otherwise) and connection/pool ownership are implementation choices for `/to-spec`. |

Unchanged and still open upstream: SC-OPEN-1 through SC-OPEN-5. Nothing here
resolves or depends on resolving them.

---

## 16. Out of scope

Unchanged from the pre-grill scope. This design does not touch: Axum or HTTP,
authentication, React, Tauri, payslip rendering, PDF, ETX, SSC filing,
tax-period reporting, accounting export, leave, overtime, bonuses, loans,
voluntary deductions, concurrent employment calculation, non-monthly payroll,
off-cycle payroll, the remuneration-classification UI, resolving SC-OPEN-1
through SC-OPEN-5, workflow engines, repository frameworks, event sourcing, or
microservices.

The result is testable entirely from Rust plus PostgreSQL.

---

## 17. Decision log

| # | Decision |
| --- | --- |
| 1 | Cargo workspace; `crates/payroll` stays pure, `crates/payroll-app` owns PostgreSQL |
| 2 | No `Reviewed` state; lifecycle is Draft → Calculated → Finalized |
| 3 | Initial finalization is atomic across the whole run |
| 4 | Membership is auto-proposed at run creation; removal is explicit |
| 5 | `PayrollRun` carries `pay_date`; it is never a calculation input |
| 6 | `PeriodsElapsed` is derived in the pure crate from the period end date |
| 7 | `OpeningBalance` freezes on the first finalization in its TaxYear |
| 8 | Ordinary payroll finalizes in pay-period order; gaps refuse |
| 9 | Finalization recomputes and requires equality with the working calculation |
| 10 | Liveness is a separate table; `finalized_payroll` has no `UPDATE` grant |
| 11 | A replacement lives in a new Correction run |
| 12 | Year-to-date sums numeric columns, never JSONB |
| 13 | `SaltVersion` is semver + git SHA; dirty-tree release builds fail |
| 14 | Removing an Employment from a run requires a reason |
| 15 | Run status is a stored column and the lock target |
| 16 | A reasoned removal from a finalized run resolves that period |
| 17 | Working recalculations are not logged |
| 18 | A reversal cannot be undone |
| 19 | A period may be corrected after later periods are finalized |
| 20 | Pure-crate ids stay opaque `TEXT`; store-owned ids are native `uuid` |
| 21 | Replacement lineage is an explicit `replaces_finalized_payroll_id` |
| 22 | An explicit `OpeningBalance` row is required, zeros included |
| 23 | `snapshot_schema_version` ships from day one |
| 24 | One stateful crate; no use-case/SQL split, no repository seam |
| 25 | One mutable `PaySchedule` per Employer; changes refuse inside a finalized TaxYear |
| 26 | `CompensationTerms` freeze once live finalized payroll references them |
| 27 | `Employment` is never deleted; mis-created ones are voided |
| 28 | Actors are bare `TEXT`; no user model in this scope |
