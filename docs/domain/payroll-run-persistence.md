# Salt — Payroll Run Persistence, Finalization, Reversal and YTD Reconstruction

**Status:** Settled by `/grill-with-docs`, 2026-08-27. Surgically re-grilled the
same day: complete `PayrollInput` facts, `OpeningBalance` coverage,
three-way finalization equality, correcting historical facts, correction-run
membership, and sequencing. 40 decisions, 7 new ADRs, 2 amended.
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

One inherited constraint **was** reopened, deliberately, and is recorded as
withdrawn: an earlier round of this grill froze `CompensationTerms` once live
finalized payroll referenced them. §6 explains why that was wrong and what
replaces it. Nothing else in the table above moved.

Seven new ADRs record the rationale this grill produced:

| ADR | Decision |
| --- | --- |
| ADR-0009 | The stateful layer is one crate with no repository seam |
| ADR-0010 | Finalization is whole-run, recomputed, and compared to what was approved |
| ADR-0011 | Liveness is a separate table, so finalized payroll is never updatable |
| ADR-0012 | Year-to-date sums real columns, never the frozen snapshots |
| ADR-0013 | Facts later periods re-read freeze; facts consumed once do not |
| ADR-0014 | Salt's coverage of an Employment begins at an explicit, frozen boundary |
| ADR-0015 | Corrections are one Employment to a run, in an explicit chain |

Two existing ADRs are amended: **ADR-0010** now compares all three frozen
values rather than the calculation alone (§5.2), and **ADR-0004** now says
plainly that the frozen snapshot is the *sole* explainer of a historical
payroll — which is what licenses master data to stay correctable (§6.5).

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

### 4.0 The re-read rule — which facts freeze

One rule decides, for every fact below, whether it becomes immutable at
finalization. It is stated once here because otherwise the answers look
arbitrary and someone will "fix" the inconsistency (ADR-0013):

> A fact that **later periods re-read** freezes at finalization.
> A fact that is **consumed once and frozen into the snapshot** does not.

`OpeningBalance` is re-read by every later period's year-to-date (§8), so an
edit after finalization silently changes future PAYE while the snapshots still
show the old figure. It freezes. `PriorEmployment` is re-read the same way, for
the same reason, and freezes with it.

`CompensationTerms` and `UnsupportedDeductionStatus` are read once, for one
period, and frozen into that period's `PayrollInput`. Nothing later re-reads
them. Freezing the master rows as well buys nothing ADR-0004 does not already
buy, and it costs the ability to correct a fact that was captured wrong (§6.5).

Run-scoped facts — membership, earnings, the working calculation — are neither
re-read nor master data. They are working state until the run finalizes, and
the snapshot after.

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

**"The current TaxYear" is the caller's claim, so it is not the only guard.**
`change_pay_schedule` has no clock — it takes the TaxYear as a parameter, like
every other use case here — so the refusal is stated twice, from two directions:

- The refusal reads "finalized in that TaxYear **or any later one**", which
  disproves a claim naming a TaxYear earlier than one already finalized.
- A claim naming a *later* TaxYear cannot be disproved without a clock, so the
  invariant is held where the harm would land instead:
  `CreateOrdinaryPayrollRun` refuses a run whose TaxYear holds a
  `FinalizedPayroll` at a period end the current schedule does not generate. A
  schedule moved under a part-finalized TaxYear therefore buys nothing — no
  further period of that year can be run at all.

An **unfinalized `PayrollRun`** also refuses the change, in *any* TaxYear — the
claim does not narrow this one. Its period was cut by the schedule in force when
it was created, and finalization re-derives everything from the current one
(§5.1), so a change under it would strand the run outright: finalization would
refuse, and `CreateOrdinaryPayrollRun` would refuse the old period too, leaving
a run that can be neither finished nor re-made. A run that exists is a fact, so
this refusal rests on a fact rather than on the caller's account of the date.

**A change must also strand no boundary already stored.** The `employer` table
holds one `PaySchedule` and no history, so a change is retroactive for
everything not already frozen into a `PayrollInput`. The finalization refusal
protects the periods that have been paid; a second refusal protects the dated
facts standing ready for the periods that have not. A `SaltCoverageStart`
(§4.5), a `CompensationTerms` `effective_from` (§4.4) and an
`UnsupportedDeductionStatus` `effective_from` (§4.5c) are each pinned to a
boundary the schedule generates, so a change that would leave one of them on a
date the new schedule does not place is refused and names the fact and the
date. Only boundaries from the named TaxYear onward are checked: an earlier one
describes a period already paid, whose `FinalizedPayroll` froze the schedule
that cut it, and checking those too would make "change it from the next
TaxYear" unreachable for anyone who has ever run a payroll.


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

**`CompensationTerms` rows stay correctable forever.** They are master data, not
history, and by the re-read rule (§4.0) nothing later re-reads them. An earlier
round of this grill froze a row once live finalized payroll referenced it; §6.5
withdraws that, states the three guards that replace it, and gives the case that
broke it.

### 4.5 OpeningBalance

```text
OpeningBalance
- OpeningBalanceId
- EmploymentId, TaxYear                  UNIQUE together
- first_salt_period_end                  the coverage boundary
- prior_taxable_remuneration
- prior_paye
- created_at, created_by
- reason / source note
```

Same Employment, same Employer — never another employer's figures, which are
`PriorEmployment` (§4.5b) and are refused (SC-OPEN-4).

**An `OpeningBalance` is an affirmative payroll fact and is never
auto-created.** A previous round of this grill had Salt write a zero row when an
Employment was created. That turned "nobody entered prior year-to-date" into
"confirmed zero" — the precise conflation INV-012 and `PriorEmployment::Unknown`
exist to prevent, committed by Salt itself rather than by a user.

#### The coverage boundary

`first_salt_period_end` is **the first PayPeriod end date Salt is responsible
for** for this Employment in this TaxYear. Every period of the TaxYear ending
before it is pre-Salt and accounted for inside the figures on this row.

It is written as the *first Salt period*, not as the *last covered period*,
because that is the question §7's sequencing rule actually asks: where does
Salt's responsibility begin? Both forms carry the same information; only this
one reads as the answer to the question being asked.

Four guards, all checked when the row is written:

1. `first_salt_period_end` must be a period end the Employer's `PaySchedule`
   generates — the same demand INV-014 makes of a `CompensationTerms` start.
2. It must fall inside the row's `TaxYear`.
3. It must be on or after the Employment's first payable period end in that
   TaxYear. Salt cannot claim to have replaced a system for periods in which the
   Employment did not exist; those periods need no resolution at all (§7).
4. Non-zero prior figures over an **empty** covered span are refused — a
   contradiction on the face of the row. Zero figures over a non-empty span are
   allowed: an employee on unpaid leave, or below the threshold with nil PAYE,
   is ordinary.

#### What the boundary distinguishes

This is the pair the boundary exists to separate:

**A. Legitimate mid-year adoption.** The Employer adopts Salt in October.
`first_salt_period_end` is 31-October, so March–September are pre-Salt and their
figures are inside this row. October finalizes. Salt never demands a September
`FinalizedPayroll`, because September was never Salt's.

**B. A forgotten September.** The Employer adopted in March and ran March
through August in Salt. Their `OpeningBalance`, if one exists at all, carries
`first_salt_period_end` = 31-March, and it **froze at March's finalization**.
October's run walks back to September (§7), finds no live `FinalizedPayroll`, no
reasoned removal, and no boundary covering it — and refuses.

The distinction is not made by the wording of the boundary. It is made by the
boundary being **set once and frozen**, so case B cannot retroactively become
case A.

#### Required, and when

An `OpeningBalance` is **required exactly when there are pre-Salt periods to
account for**, and that is decided by §7's walk-back rather than by a rule of
its own. Finalizing period P demands that P's predecessor be resolved by
something affirmative — Salt's own live history, a reasoned removal, the
Employment not yet existing, or this boundary. Absence of an `OpeningBalance` is
legal only when Salt's records already cover every earlier period, which is a
positive fact Salt checks, not a silence Salt assumes.

The consequence is that a continuing employee needs **no** row in a TaxYear Salt
ran end to end. Requiring one would be eight auto-clicked "confirm zero"
checkboxes every March for an eight-person employer, which is how users learn to
stop reading confirmations — the same objection ADR-0010 raised against
`Reviewed`.

#### Freezing

**The whole row — figures and boundary together — freezes on the first
finalization for that Employment in that TaxYear.** Not the Employer's first:
an Employment onboarded in June can still be given a June boundary after other
Employments have finalized March.

By the re-read rule (§4.0) this fact is re-read into every later period's
year-to-date, so an editable balance would silently change every future period's
PAYE while the frozen snapshots still showed the old figure — the "history
quietly changed" failure ADR-0004 exists to stop. Correcting it afterwards means
reversing the finalized payroll that froze it.

`periods_elapsed` is **not** stored here. It is derived from the PayPeriod's
position in the TaxYear (§8).

### 4.5b PriorEmployment

```text
PriorEmploymentDeclaration
- EmploymentId, TaxYear                  UNIQUE together
- status                                 ConfirmedNone | Present | (no row = Unknown)
- taxable_remuneration, paye             set together, only when Present
- declared_at, declared_by
```

**Employment + TaxYear state.** It is the answer to "did this Person have
taxable employment with a *different* Employer earlier in this TaxYear, and what
did it pay". Concurrent employment is out of scope, so the window it describes
closes when this Employment starts: the fact cannot change during the year.
Effective-dating it would create versions of a fact that has none, and offer a
wrong answer for March. Making it run-scoped would ask the same question twelve
times a year and let March and April disagree about it, while cumulative PAYE
depends on their agreeing.

**Absence of a row means `Unknown`, and nothing else.** This needs no
pre-finalization gate, because the calculator already refuses `Unknown`
(ADR-0001): a run whose member has no declaration cannot reach `Calculated`, so
it can never reach the finalization transaction. The type does the work.

**It freezes with the `OpeningBalance`**, on the first finalization for that
Employment in that TaxYear, by the re-read rule — it is carried into every later
period's `YearToDateContext`. Correcting it afterwards is a reversal.

One consequence is stated rather than designed around: recording
`Present(figures)` for an Employment mid-year makes every subsequent period
refuse, because SC-OPEN-4 refuses `Some`. That is the calculator's existing and
correct behaviour. What the user is told, and what they do next, is **OPEN-P4**.

### 4.5c UnsupportedDeductionStatus

```text
UnsupportedDeductionDeclaration
- EmploymentId, effective_from           UNIQUE together
- status                                 ConfirmedNone | Present(kinds)
- kinds                                  non-empty only when Present
- declared_at, declared_by
```

**Effective-dated state**, `effective_from` only — in force until the next row,
the same shape as `CompensationTerms`, and pinned to a pay-period start date so
exactly one row governs a period.

This one genuinely changes mid-year: an employee joins a provident fund in
August, and March through July were correctly `ConfirmedNone`. Holding it as
Employment + TaxYear state would mean the fund could not be recorded without
reversing March — and nothing about March was wrong. That is the case that
decides it.

It is **another effective-dating axis**, and §4.2 was right to refuse one for
`PaySchedule`. The difference is frequency and blast radius: a `PaySchedule`
changes once a decade, only at a TaxYear boundary, and is frozen into every
`PayrollInput` anyway. This changes per employee, mid-year, and blocks that
employee's payroll when it does. The axis is not the cost — building one for a
fact that does not move is.

**No row in force at the period end means `Unknown`**, which refuses in the
calculator exactly as §4.5b's absence does. An empty `kinds` collection is not
representable (`unsupported_deduction.rs`), so `Present` can never collapse into
`ConfirmedNone` by accident.

**It does not freeze** (§4.0): it is a gate read once per period and frozen into
that period's snapshot. Corrections follow §6.5.

### 4.5d Run earnings

```text
PayrollRunEarning
- PayrollRunId, EmploymentId, line
- the classified Earning
```

**Run-scoped**, because that is what an allowance is: a fact about paying this
Employment for this period. `BasicPay` is never here — `calculate` adds it from
the `CompensationTerms` and refuses a second one (`calculation.rs:37`).

**Absence means no additional earnings, and that is a complete statement.** This
is a deliberate asymmetry with §4.5b and §4.5c, and the reason is the difference
between the facts. Those two are three-valued because a **statutory** question
may go unasked, and Salt must never answer it by default. Earnings are the
Employer's own act of paying; there is no unasked question and nothing to
confirm-none. A per-employee, per-month "no allowances this period" checkbox
would fire for every employee in every run and enforce no invariant — the
ceremony ADR-0010 rejected.

A Correction run's earnings are pre-populated from the reversed
`FinalizedPayroll`'s frozen `payroll_input_json` and then edited (§6.3). Where
that snapshot's `snapshot_schema_version` is not one the running Salt
deserializes, the run starts with no earning lines and says so. ADR-0004
promises history is explainable, never that every old snapshot deserializes
forever, so the convenience has to degrade honestly rather than pretend.

### 4.6 PayrollRun

```text
PayrollRun
- PayrollRunId
- EmployerId
- PayPeriod
- pay_date
- kind      Ordinary | Correction
- status    Draft | Calculated | Finalized
- correction_reason   mandatory and non-empty when kind = Correction, else null
- created_at, created_by
```

`pay_date` lives here. `CONTEXT.md` already promised it, ADR-0005 expects it
when statutory reporting is built, and it is **never a calculation input** — it
does not reach `PayrollInput`.

**Uniqueness applies to `Ordinary` only:** one Ordinary run per Employer +
PayPeriod. Correction runs are unconstrained in number, because a period may
legitimately be corrected more than once — but each holds exactly one
Employment (§4.8).

`status` is a **stored column**, not derived. The finalization transaction takes
`SELECT … FOR UPDATE` on this row. A derived status offers nothing to lock,
which is precisely how two processes both conclude they may finalize.

### 4.7 PayrollRun lifecycle

```text
Draft  ⇄  Calculated  →  Finalized
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

**`Calculated` is editable, and editing reopens the run.** The arrow back to
`Draft` is not a second thought about the lifecycle: `Calculated` is defined
above as a *property of the members*, not as a gate the Employer passed
through. Seeing the figures is precisely when a wrong Earning, or a member
who should not be paid this period, becomes visible — so refusing the edit
would leave an Employer who spotted a mistake with finalizing it or nothing.
The instant an Earning changes or a member leaves, the stored calculations
stop being a current account of the run, which is the definition of `Draft`;
moving the status back is what keeps the column honest rather than a claim
about calculations that have gone stale. The Employer recalculates to reach
`Calculated` again.

A separate `ReopenPayrollRun` act was rejected for the reason §4.7 rejects
`Reviewed`: it enforces no invariant the edit does not already enforce, and
buys a second thing to forget. `Finalized` remains the one absolute refusal.

### 4.8 Run membership

```text
PayrollRunEmployment
- PayrollRunId, EmploymentId
- removed_at, removed_by, removal_reason        (Ordinary only; nullable, set together)
- replaces_finalized_payroll_id                 (Correction only; the declared target)
```

**Membership semantics differ by `kind`, and they are opposites.**

#### Ordinary — auto-proposed, removal is the deliberate act

Creating a run writes a membership row for **every Employment overlapping the
PayPeriod**. A user may remove one before calculation, and **a removal requires
a non-empty reason**.

Silent omission is the dangerous failure — an employee nobody noticed was not
paid. Auto-inclusion makes omission a deliberate, reasoned, logged act, and the
reason is the thing the employer needs six months later.

#### Correction — nothing is proposed, inclusion is the deliberate act

**A Correction run holds exactly one Employment**, added explicitly. Nothing is
auto-proposed: sweeping in every Employment that happened to be employed in an
old period is how a one-employee fix becomes a nine-employee re-issue.

One rather than several, because ADR-0002 already made corrections per
Employment (ADR-0015). A multi-member Correction run would re-import the
whole-run atomicity question of §5.1 into an act deliberately kept per
Employment, and would make one refusal block four unrelated fixes. The cost is
that an Employer correcting five employees for one bad rate creates five runs —
which is honest, because it is five reversals and five replacements either way.

#### Correction lineage

`replaces_finalized_payroll_id` is set **exactly when a reversed predecessor
exists** for this `(employment_id, period_end)`.

**It appears on two tables, and they are not the same fact.** On membership it
is the *declared target* of a run that has not happened yet — working state,
changeable while the run is `Draft`, and abandoned entirely if the run is. On
`FinalizedPayroll` (§9) it is the *recorded lineage* of a correction that did
happen, copied across inside the finalization transaction and never updated.
**The UNIQUE constraint belongs on the `FinalizedPayroll` copy**, not on
membership: two draft Correction runs may both name F1 as their target, and the
first to finalize wins — which is the same shape as the liveness primary key in
§6.2, and it fails at the same moment for the same reason.

Because it is UNIQUE there, a given `FinalizedPayroll` is replaced at most once
and repeated corrections form an unambiguous chain rather than a tree:

```text
F1  →  reversed  →  F2 (replaces F1)  →  reversed  →  F3 (replaces F2)
```

The target must be reversed, not live, and must match this Employment, Employer
and period end. Inference from `(employment_id, period_end)` ordered by
`finalized_at` is an argument that collapses the second time a period is
corrected; a foreign key answers outright.

**The column is null in two legitimate cases**, and this is the one place where
a Correction run has nothing to replace:

- the Employment was **removed with a reason** from the finalized Ordinary run
  for that period, and the removal turns out to have been wrong;
- the Employment was **never a member at all** — created later, with a backdated
  `start_date`.

In both, there is no `FinalizedPayroll` to reverse, and §4.6 forbids a second
Ordinary run for the period. Refusing here would mean the employee can never be
paid for that period at all; paying them in a later ordinary period instead
would attribute the money to the wrong PayPeriod and scale their PAYE bands
against the wrong `period_number` (ADR-0001). The run's mandatory
`correction_reason` carries what happened.

That is why lineage is not a simple biconditional in SQL. The database enforces
what it can — UNIQUE, the foreign key, the matching employment and period, and
`correction_reason` non-empty when `kind = Correction`. Rust enforces "set
exactly when a reversed predecessor exists", which is a domain question. §11
already draws the split there.

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

### 5.2 Reassemble, re-resolve, recompute — and require all three to match

Finalization does not merely recalculate. It **rebuilds every one of the three
values the working calculation stores**, by the same paths that built them the
first time, and refuses unless all three are equal:

```text
assemble PayrollInput from current facts        ==  stored PayrollInput
resolve  PayrollRules via ruleset_for(period.end) ==  stored PayrollRules
calculate(input, rules)                          ==  stored PayrollCalculation
```

```text
current facts → assemble → resolve → calculate
      ↓             ↓          ↓          ↓
   all three must equal what was approved  →  finalize
```

This establishes:

> The calculation that becomes history is exactly the one the user approved on
> screen — the same numbers, from the same facts, under the same rules.

**Comparing only the calculation is not enough**, and the failure is specific.
A `FinalizedPayroll` freezes the input and the rules as well as the result
(ADR-0004), and all three are what a future reader is shown. Suppose a PAYE
table is corrected in a band this employee never reaches, or a
`CompensationTerms` `effective_from` is fixed without touching `BasicPay`. The
monetary output is identical to the cent. Under an output-only comparison, Salt
would write a `payroll_rules_json` or a `payroll_input_json` into permanent
history that nobody ever approved and no screen ever showed. The result would be
right and the explanation would be a fabrication — which breaks INV-005 for a
row that looks perfectly correct.

Requiring equality on all three costs nothing. `PayrollInput`
(`calculation.rs:33`), `PayrollRules` (`rules.rs:659`) and `PayrollCalculation`
(`calculation.rs:383`) each already derive `PartialEq + Eq`. It is three `==`,
not one.

**A mismatch is not an error to route around: it is the mechanism working.** The
refusal must name **which of the three** differed and what changed inside it,
and demand a fresh calculation the user looks at again. A refusal that says only
"something changed" is one users learn to click past.

This is why no fingerprint, revision or staleness-detection machinery is needed
on the working calculation. Reassembling, re-resolving and recomputing *is* the
staleness check, across all three axes at once, and it cannot miss a dependency
nobody thought to fingerprint.

### 5.3 The transaction

```text
BEGIN
 1. SELECT … FOR UPDATE the PayrollRun row
 2. verify status is Calculated and kind/period are as expected
 3. per kind:
      Ordinary   — verify the previous period is resolved for every member (§7)
      Correction — verify the single member's target is reversed and not live,
                   or that its null-lineage case holds (§4.8); check no
                   other period, earlier or later (§7.4)
 4. for every member, rebuild all three:
      assemble PayrollInput   from current facts     == stored PayrollInput
      resolve  PayrollRules   via ruleset_for(end)   == stored PayrollRules
      calculate(input, rules)                        == stored PayrollCalculation
    refuse on any inequality, naming which of the three and what changed
 5. INSERT the immutable FinalizedPayroll rows
 6. INSERT the live_finalized_payroll rows        ← the concurrency guard
 7. INSERT the action-log entries
 8. UPDATE payroll_run SET status = 'Finalized'
COMMIT
```

Any failure rolls back. There is no partial history, and no path writes a
`FinalizedPayroll` without its liveness row and its audit entry.

**The old step 3 is gone.** It verified that every member had an
`OpeningBalance` for the TaxYear. That check was the auto-created zero row's
partner, and §4.5 retired both: an `OpeningBalance` is now required exactly when
§7's walk-back needs one to resolve an earlier period, and the walk-back is the
check. `PriorEmployment` and `UnsupportedDeductionStatus` need no gate here
either — absence of either declaration yields `Unknown`, `Unknown` refuses
inside `calculate`, and a run that cannot calculate never reaches `Calculated`
(§4.7). The type keeps the transaction short.

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

The same reasoning covers the two guards §4.0 and §4.2 add, and both are
read-then-write, so both need a lock rather than an isolation level:

- **The freeze.** An edit of a frozen fact takes `FOR UPDATE` on the
  `employment` row; finalization takes `FOR SHARE` on every member's before it
  reads any master data. The two conflict, so the loser waits and then sees the
  winner's committed result — the edit refuses because a `FinalizedPayroll` now
  exists, or the finalization refuses because the fact it re-reads no longer
  matches the approved one (§5.2).
- **The schedule.** `ChangePaySchedule` takes `FOR UPDATE` on the `employer`
  row; everything that reads the schedule to validate a date against it —
  finalization, run creation, and each of the three use cases that *store* a
  schedule-bounded boundary — takes `FOR SHARE` on the same row. Without that
  last one the stranded-boundary refusal above is decorative: the writer would
  validate against the schedule its snapshot shows, the changer would look for
  stored boundaries before the writer's row was visible, and both would commit.

**Lock order is `employer` before `employment`, everywhere.** That is the only
reason these two rules do not deadlock against each other: run creation locks
the `employer` row and then reaches `employment` through its membership
insert's foreign key, so a fact writer that needs both must take the `employer`
lock in its own statement first rather than joining the two tables in one.

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
same PayPeriod, holding exactly one Employment (§4.8). Reopening the original
run would mutate a finalized run, which §5.1 forbids, and a correction has its
own pay date, its own mandatory reason and its own audit trail because it is
genuinely its own event.

Lineage is **explicit**: `replaces_finalized_payroll_id`, set at insert, never
updated, UNIQUE, and null only in the two cases §4.8 names. §13's success
condition is what an auditor can inspect, and a foreign key answers "what did
this replace" outright.

The correction's earnings are pre-populated from the reversed
`FinalizedPayroll`'s frozen input and then edited (§4.5d). Its `PayrollInput` is
otherwise **assembled from current master data by the ordinary path** — which is
what §6.5 exists to make possible.

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

### 6.5 Where a correction gets corrected facts

A replacement calculation has to come from somewhere. This section resolves the
conflict between "master data is frozen once payroll references it" and
"reversal plus replacement must be able to calculate from corrected historical
facts". They cannot both hold, and the freeze is the one that goes.

#### The case that breaks the freeze

March, April and May all reference one `CompensationTerms` row. All three are
finalized. In June, March's underlying salary fact is found wrong. ADR-0002 lets
March be reversed without cascading into April and May — but April and May keep
that same row live, so under the old rule it is frozen and March has no
corrected fact to calculate from.

Note what the truth actually is. If the terms genuinely applied unchanged from
March to May and were wrong, then April and May are wrong too. The case where
**only March** is wrong necessarily means the terms differed in March — so the
honest repair is to **split the row**: March at the correct amount, and the
existing row's `effective_from` moved to 1-April. That leaves April and May
using the identical `BasicPay` they always used. But it moves a frozen
`effective_from`, so the old rule forbids the one edit that states the truth.

#### The decision

**`CompensationTerms` and `UnsupportedDeductionStatus` rows are correctable
master data, forever.** Decision 26 is withdrawn (ADR-0013). Three guards
replace it:

1. **Every change is audited.** A `CompensationTermsCorrected` or
   `UnsupportedDeductionStatusCorrected` action-log entry carries a mandatory
   non-empty reason, the before and after values, and the actor.
2. **Divergence is named, not hidden.** When a change touches a span holding any
   live `FinalizedPayroll`, Salt lists **every affected finalized period** and
   requires the user to acknowledge that master data now differs from history.
   The same action-log entry carries that list. It is a warning, never a
   refusal, and it never touches a finalized row.
3. **Nothing arithmetic can move.** Year-to-date sums frozen numeric columns
   (ADR-0012), never master data, so a master edit cannot change a cent of any
   finalized figure. Only a reversal plus a replacement can.

The acknowledgement lives in the action log and nowhere else. A separate
sign-off table would be a second place to say one thing, which §7.1 already
rejects by name for `NoPayrollRecord`.

#### Why this does not reopen ADR-0004

It sharpens it. ADR-0004 already settled who explains a historical payroll: the
**frozen snapshot**, never current master data or current rule code. Freezing
the master row as well was belt-and-braces that bought nothing ADR-0004 did not
already guarantee, and it cost the ability to correct a fact that was captured
wrong. ADR-0004 is amended to say the snapshot is the *sole* explainer, which is
precisely what licenses master data to stay editable.

The §13 worry — "the frozen payslip and the history screen tell two different
stories" — is real, and the answer is that they are labelled as two different
things and the difference is *explained*. The payslip renders from the snapshot.
The employment history screen shows master data **with its audit trail**, so a
reader sees "BasicPay corrected on 2026-06-14 by <actor>, reason: March rate
captured wrong". A visible, attributed, reasoned difference is not the failure
ADR-0004 guards against; a silent one is.

#### Why not the alternatives

**Versioned or superseding `CompensationTerms`** adds a fourth axis on top of
the three this design already carries, and it does not even solve the problem:
April and May would then be explained by a row marked superseded, which is the
same divergence in a more expensive shape.

**Correction-run input overrides** put the corrected fact only inside the
correction run, so master data keeps stating the wrong salary forever. That is
the divergence again, inverted, and with no audit trail on the master row at
all.

#### What does not change

`OpeningBalance` and `PriorEmployment` still freeze (§4.5, §4.5b). This is not
an inconsistency, it is the re-read rule (§4.0): those two are read fresh into
**every** later period's year-to-date, so editing them after finalization
silently re-prices the future. `CompensationTerms` and
`UnsupportedDeductionStatus` are consumed once, for one period, and frozen into
that period's snapshot. Editing them changes nothing that has already happened.

#### The March correction, end to end

1. Correct master data: split the `CompensationTerms` row so March carries the
   true `BasicPay`, with a reason. Salt names March, April and May as live
   finalized periods now diverging from master; the user acknowledges. April and
   May's `BasicPay` is unchanged by the split, so only March genuinely diverges.
2. Reverse March's `FinalizedPayroll`, with a reason. Its liveness row is
   deleted.
3. Create a Correction run for March holding only that Employment, naming the
   reversed row as `replaces_finalized_payroll_id`, with a
   `correction_reason`.
4. Calculate. `PayrollInput` is assembled from current master data by the
   ordinary path and now carries the corrected March figure. Earnings are
   pre-populated from the reversed snapshot.
5. Finalize. §5.2 reassembles, re-resolves and recomputes all three, and §7.4
   checks no other period. A new `FinalizedPayroll` and a new liveness row for
   `(employment, 31-March)` are written.
6. April and May are untouched. Their snapshots still explain themselves, and
   ADR-0001 absorbs the difference at the next calculation.

---

## 7. Ordering and sequencing

Payroll order is **`PayPeriod.end_date`**, never `finalized_at`. A
late-finalized March is still March, and a replacement finalized in October is
still the March figure for year-to-date purposes.

### 7.1 When a period is resolved

This is the whole definition. Everything else in this section follows from it.

> For Employment **E** and PayPeriod **P** in TaxYear **Y**, P is **resolved**
> for E when at least one of these holds:
>
> 1. **Outside the Employment** — P does not overlap
>    `[E.start_date, E.end_date]`. Nothing was owed.
> 2. **Before Salt** — an `OpeningBalance` exists for `(E, Y)` and
>    `P.end < OpeningBalance.first_salt_period_end`. P's figures are inside that
>    balance (§4.5).
> 3. **Paid** — a live `FinalizedPayroll` exists for `(E, P.end)`.
> 4. **Explicitly not paid** — either E was removed with a mandatory reason from
>    the finalized Ordinary run for P (§4.8), or every `FinalizedPayroll` Salt
>    holds for `(E, P.end)` has been reversed and none is live.
>
> **Nothing else resolves a period. Absence of a record never resolves one.**

Branch 4's second half is the one worth stating plainly: **a bare reversal
resolves the period.** ADR-0002 says a reversal with no replacement is valid and
arithmetically complete — someone was paid who should not have been — and if it
did not resolve the period, every later run in the year would refuse and
ADR-0002 would be unusable in practice.

That does not weaken "payroll gaps must never silently disappear". A `Reversal`
is not a gap and is not silent: it is an immutable record with a mandatory
reason and a named actor, which is exactly what a reasoned removal is in a
different shape. There is still deliberately **no separate `NoPayrollRecord`
concept** — the reversal and the reasoned removal already record the fact, and a
third way to say one thing is how all three drift apart.

### 7.2 What an Ordinary run checks

**An Ordinary run for period P refuses to finalize unless the immediately
preceding period of the same TaxYear is resolved for every member.**

Cumulative PAYE is *wrong*, not merely incomplete, when a period silently
vanishes. Refusing converts silent under-withholding into a visible error.

**One period back is sufficient, and this is why.** Resolution is inductive —
if P's predecessor is resolved, every earlier period of Y is resolved:

- Branch **1** and branch **2** each cover everything earlier by construction:
  an Employment's start date does not move backwards, and every period before
  the boundary is inside the same balance.
- Branch **3**, branch **4** — that period was itself finalized, and its
  finalization ran this very check against *its* predecessor.

And nothing can un-resolve a period afterwards: a reversal still resolves it
(branch 4), a removal is immutable, and the `OpeningBalance` boundary froze at
the Employment's first finalization in that TaxYear (§4.5). Walking eleven
periods back would re-prove what the first lookup already proves.

**The walk stops at the TaxYear's first period.** Nothing before it is checked.
PAYE is cumulative within a TaxYear and resets at the boundary (ADR-0001), so a
2025/26 that Salt never touched cannot block 2026/27 — that year simply opens
with zeros, or with an `OpeningBalance` if there are pre-Salt periods inside it.

### 7.3 The cases, worked

| Case | Resolved by |
| --- | --- |
| Continuing employee, March finalized, April running | branch 3 — live March payroll |
| New employee starting 1-October; September checked | branch 1 — outside the Employment |
| Employer adopts in October; September checked | branch 2 — boundary is 31-October |
| Employer adopted in March, ran to August, forgot September | **nothing** — refuses |
| On unpaid leave, removed from March's finalized run with a reason | branch 4 — reasoned removal |
| March reversed in June with no replacement; July running | branch 4 — reversed, none live |
| Employment onboarded in June, `start_date` in March | needs an `OpeningBalance` with a June boundary, or the March–May periods back-filled |

Rows four and three are the pair §4.5 exists to separate, and the separation is
made by the boundary being frozen, not by its wording.

### 7.4 What a Correction run checks

**A Correction run checks its own period and nothing else.**

It verifies that its single member's target is reversed and not live, or that
one of §4.8's two null-lineage cases holds. It does **not** walk back, and it
does **not** look forward.

Not backwards, because those periods were checked when the original finalized
and §7.2's induction shows they stay resolved. Not forwards, because §6.4
already settled that a period may be corrected after later periods are
finalized. Under a backward check, an unrelated reversal somewhere else in the
year would block an unrelated fix — turning the sequencing rule from a guard
against silent gaps into an obstacle to closing them.

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

prior_employment =
    the PriorEmploymentDeclaration for (Employment, TaxYear)
    — no row means Unknown, and Unknown refuses   (§4.5b)
```

**Where the `OpeningBalance` terms come from when there is no row.** By §4.5 a
row exists only where there are pre-Salt periods to account for. Where there is
none, `prior_taxable_remuneration` and `prior_paye` contribute **zero** — and
that zero is not an assumption from silence. §7's walk-back has already proved
that Salt's own live history, a reasoned removal, or the Employment not yet
existing covers every earlier period of the TaxYear. The zero is a consequence
of a checked fact, not a default.

`prior_employment` is the one term with no such fallback, and it needs none: an
absent declaration yields `Unknown`, `calculate` refuses `Unknown`, and the run
never reaches `Calculated` (§4.5b).

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

`payroll_input_json` is the complete `PayrollInput` — which now includes the
`PriorEmployment` fact, the `UnsupportedDeductionStatus` and the run's earning
lines, because those are fields of the type (`calculation.rs:34`). That is the
point of §4.5b through §4.5d: every field of `PayrollInput` has exactly one
persisted source, so the stateful layer never has to invent one.

`replaces_finalized_payroll_id` is copied here from the Correction run's
membership row inside the finalization transaction, and it is **UNIQUE on this
table** (§4.8) — which is what makes a repeated correction a chain rather than a
tree, and what makes two draft runs racing for the same target fail the way §6.2
makes duplicate liveness fail.

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
`EmploymentAddedToCorrectionRun` (with the run's `correction_reason` and its
target, if any), `PayrollFinalized`, `FinalizedPayrollReversed`,
`OpeningBalanceCreated`, `OpeningBalanceChanged`,
`PriorEmploymentDeclared`, `PriorEmploymentChanged`,
`CompensationTermsCorrected`, `UnsupportedDeductionStatusCorrected`,
`PayScheduleChanged`, `EmploymentVoided`.

The two `*Corrected` entries are load-bearing rather than informational (§6.5).
Each carries a mandatory reason, the before and after values, and **the list of
live finalized periods the change now diverges from**. That list is the whole
acknowledgement mechanism: there is no separate sign-off table, because a second
place to record one fact is how the two drift apart — the same objection §7.1
makes to a third way of saying "not paid".

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
UNIQUE (employment_id, tax_year)              one PriorEmploymentDeclaration
UNIQUE (employment_id, effective_from)        one UnsupportedDeductionDeclaration
UNIQUE (payroll_run_id, employment_id)        one working calculation
UNIQUE (finalized_payroll_id) on reversal     one reversal
UNIQUE (replaces_finalized_payroll_id) on finalized_payroll, not on membership
PK (employment_id, period_end) on liveness    one live payroll
one Ordinary run per Employer + PayPeriod
CHECK  correction_reason non-empty when kind = Correction
CHECK  at most one membership row when kind = Correction
FK     no voided Employment in run membership
REVOKE UPDATE, DELETE on finalized_payroll
```

**Rust** — anything requiring domain reasoning:

```text
Employment overlaps the PayPeriod
applicable CompensationTerms
applicable UnsupportedDeductionDeclaration for the period
calculator support and refusals
lifecycle transitions
period-resolved checks, per run kind             (§7)
OpeningBalance boundary guards                   (§4.5)
lineage set exactly when a reversed predecessor exists   (§4.8)
reassemble / re-resolve / recompute and compare all three (§5.2)
divergence listing on a master-data correction   (§6.5)
```

The lineage rule is the clearest example of the split. SQL can enforce that
`replaces_finalized_payroll_id` is unique, references a real row, and matches
this Employment and period. Only Rust can decide whether a reversed predecessor
*exists* — and therefore whether the column may legitimately be null.

Not every domain rule is duplicated in SQL. No concurrency-safe structural
invariant is left to application memory.

---

## 12. Application seams

Explicit functions, not a framework:

```text
CreateOrdinaryPayrollRun    CreateCorrectionRun
AddEmploymentToPayrollRun   RemoveEmploymentFromRun
CalculatePayrollRun         FinalizePayrollRun
ReverseFinalizedPayroll     GetPayrollRun
GetFinalizedPayroll         BuildYearToDateContext

RecordOpeningBalance                  DeclarePriorEmployment
DeclareUnsupportedDeductionStatus     CorrectCompensationTerms
SetRunEarnings
```

`CreateOrdinaryPayrollRun` and `CreateCorrectionRun` are separate functions
rather than one taking a `kind`, because §4.8 gives them opposite membership
semantics: one auto-proposes every overlapping Employment, the other proposes
nothing and takes exactly one. A single entry point would have to branch on
`kind` in its first line and share nothing after it.

`CorrectCompensationTerms` is named as its own use case, not folded into a
generic master-data update, because §6.5 makes it a payroll act: it must gather
a reason, compute the list of live finalized periods the change diverges from,
and write the action-log entry carrying both.

No `Repository<T>`, no `UnitOfWork`, no `Specification<T>`. Explicit SQLx
transactions. Abstraction arrives when there are two real implementations.

---

## 13. The success conditions, answered

**An employer finalizes March. April builds year-to-date from it. Months later
March is found wrong, reversed and replaced. What exists?**

Rows: the original `FinalizedPayroll` (untouched), a `Reversal` naming it with a
mandatory reason, and a replacement `FinalizedPayroll` in a single-Employment
Correction run whose `replaces_finalized_payroll_id` names the original and
whose `correction_reason` says why. All three are immutable.
`live_finalized_payroll` for `(employment, 31-March)` pointed at the original,
was deleted by the reversal, and now points at the replacement. A newly-built
May year-to-date sees the replacement only — it joins through liveness and never
mentions reversals. April's own finalized snapshot is untouched; ADR-0001
absorbs the difference at the next calculation. An auditor inspects three
immutable rows, the frozen input, rules and result on each, and the action-log
entries that committed with them — including the
`CompensationTermsCorrected` entry that supplied the corrected fact, with its
reason and the periods it diverged from.

**An employer adopts Salt in October. Why does October finalize, and why does a
forgotten September not?**

Both questions are answered by one frozen date. In the adoption case the
Employment's `OpeningBalance` for the TaxYear carries
`first_salt_period_end` = 31-October and the March–September figures; §7.1
branch 2 resolves September, and October finalizes. In the forgotten case the
Employer adopted in March and ran through August, so their boundary — if a row
exists at all — reads 31-March and froze at March's finalization. October's
walk-back finds no live September payroll, no reasoned removal and no boundary
covering September, and refuses. The wording of the boundary distinguishes
nothing; the boundary being **set once and frozen** distinguishes everything.

**March, April and May share one `CompensationTerms` row, and March's salary
fact was wrong. Where does the replacement get corrected facts?**

From master data, by the ordinary path. The `CompensationTerms` row is split so
March carries the true amount and the existing row starts 1-April, with a
mandatory reason; Salt names the live finalized periods now diverging and the
user acknowledges. March is reversed; a Correction run holding only that
Employment reassembles `PayrollInput` from current master data, recomputes, and
finalizes as the replacement. April and May keep the identical `BasicPay` they
always had, their snapshots are untouched, and no finalized figure moved,
because year-to-date sums frozen numeric columns rather than master data. Master
data was correctable **because** ADR-0004 already made the snapshot the sole
explainer of history (§6.5).

**Someone was wrongly removed from March's finalized run. Can they ever be
paid?**

Yes, and this is the one Correction with nothing to replace. §4.6 forbids a
second Ordinary run for March, and no `FinalizedPayroll` exists to reverse, so
the Correction run's `replaces_finalized_payroll_id` is null and its
`correction_reason` carries what happened (§4.8). Paying the money in a later
ordinary period instead would attribute it to the wrong PayPeriod and scale
their PAYE bands against the wrong `period_number`.

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
rebuilt from current employee records or current rule code.

That is the *whole* answer, and §6.5 leans on it being the whole answer.
`CompensationTerms` may since have been corrected, and March is still explained,
because the snapshot never referred to that row — it froze its values. The
employment history screen shows current master data with its audit trail, so a
difference between the two reads as "corrected on this date, by this actor, for
this reason", not as a contradiction. What ADR-0004 forbids is history changing
*silently*; a named, reasoned, attributed difference is the opposite of that.

---

## 14. Tests this design owes

The tracer bullet — the one that proves the architecture:

```text
GIVEN  Employer E, monthly PaySchedule, Employment A starting 1-March,
       CompensationTerms, a PriorEmploymentDeclaration of ConfirmedNone,
       an UnsupportedDeductionDeclaration of ConfirmedNone from 1-March,
       and NO OpeningBalance — March is A's first payable period, so §7.1
       branch 1 resolves everything earlier and no balance is required
WHEN   a March Ordinary run is created, A included, calculated, finalized
THEN   FinalizedPayroll freezes input, rules, result, both rule ids,
       snapshot schema version and Salt version
WHEN   an April run is created and calculated
THEN   April's YearToDateContext = zero opening terms + live March FinalizedPayroll
AND    April's prior taxable remuneration and prior PAYE equal March's
       finalized outputs
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
9. later Employment or `CompensationTerms` changes cannot alter frozen history;
10. year-to-date orders by pay period, not finalization timestamp;
11. `periods_elapsed` is never derived by counting rows;
12. finalizing April refuses while March is unresolved;
13. an `UPDATE` on `finalized_payroll` is refused by the database role itself.

**Complete `PayrollInput` facts** (§4.5b–§4.5d):

14. a missing `PriorEmploymentDeclaration` yields `Unknown`, so the run refuses
    at calculation and never reaches `Calculated`;
15. the same for a period with no `UnsupportedDeductionDeclaration` in force;
16. an August `Present(ProvidentFund)` declaration refuses August onward and
    leaves March–July finalized and untouched — no reversal required;
17. `OpeningBalance` and `PriorEmploymentDeclaration` both refuse edits after
    that Employment's first finalization in the TaxYear;
18. a `PriorEmploymentDeclaration` freeze is per Employment, not per Employer:
    an Employment onboarded in June can still be declared after another
    Employment finalized March.

**`OpeningBalance` coverage** (§4.5, §7):

19. no `OpeningBalance` is auto-created when an Employment is created;
20. a boundary that is not a period end the `PaySchedule` generates is refused;
21. a boundary before the Employment's first payable period is refused;
22. non-zero prior figures over an empty covered span are refused;
23. **the A/B pair** — an October boundary lets October finalize with
    September unresolved by any Salt record, while a frozen March boundary
    makes an October run refuse for a skipped September;
24. the boundary cannot be moved after that Employment's first finalization.

**Sequencing** (§7):

25. each of §7.1's four branches resolves a period, one test apiece;
26. a bare reversal resolves its period, so a later Ordinary run finalizes;
27. an Ordinary run reads exactly one preceding period, and never crosses the
    TaxYear boundary;
28. a Correction run for an old period ignores both earlier and later periods;
29. an Employment onboarded in June with a March `start_date` refuses until
    given a June boundary or back-filled.

**Three-way finalization equality** (§5.2):

30. finalization refuses when the recomputed `PayrollCalculation` differs;
31. finalization refuses when the reassembled `PayrollInput` differs **while the
    `PayrollCalculation` is byte-identical** — an `effective_from` corrected
    without touching `BasicPay`;
32. finalization refuses when the re-resolved `PayrollRules` differ **while the
    `PayrollCalculation` is byte-identical** — a PAYE band corrected outside the
    range this employee reaches;
33. the refusal names which of the three differed.

**Corrections** (§4.8, §6.5):

34. a Correction run refuses a second Employment;
35. no Employment is auto-proposed into a Correction run;
36. `replaces_finalized_payroll_id` is UNIQUE, so a reversed row cannot be
    replaced twice — repeated corrections form a chain;
37. a Correction run may be finalized with null lineage after a reasoned removal
    or an omission, and refuses without a `correction_reason`;
38. a `CompensationTerms` correction over a live finalized span succeeds, is
    logged with before and after values and the affected period list, and
    changes no finalized figure and no year-to-date total;
39. a March correction sourced from split `CompensationTerms` leaves April and
    May's snapshots and live rows byte-identical.

---

## 15. Still open

| Id | Question |
| --- | --- |
| OPEN-P1 | Actors are bare `TEXT`. When authentication arrives, `actor` becomes a foreign key — no user model is designed here. |
| OPEN-P2 | ADR-0001's below-withheld refusal becomes user-reachable through corrections (§6.4). What a user is told, and what they do next, is a product question this design does not answer. |
| OPEN-P3 | Migration tooling (`sqlx migrate` or otherwise) and connection/pool ownership are implementation choices for `/to-spec`. |
| OPEN-P4 | A `PriorEmployment` fact discovered mid-year is a stop-the-world event for that Employment: recording it needs a reversal, and `Present(figures)` then refuses every period while SC-OPEN-4 is open (§4.5b). The behaviour is correct and stays. What the user is told, and what they do next, is a product question — the same shape as OPEN-P2, and it resolves when SC-OPEN-4 does. |

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
| 7 | `OpeningBalance` freezes — figures and boundary — on that Employment's first finalization in its TaxYear |
| 8 | Ordinary payroll finalizes in pay-period order; gaps refuse |
| 9 | **Amended.** Finalization reassembles the `PayrollInput`, re-resolves the `PayrollRules` and recomputes the `PayrollCalculation`, and requires **all three** to equal the approved working calculation |
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
| 21 | **Amended.** Replacement lineage is an explicit, UNIQUE `replaces_finalized_payroll_id`, set exactly when a reversed predecessor exists, so repeated corrections form a chain |
| 22 | **Amended.** An `OpeningBalance` is an affirmative fact, never auto-created, required exactly when §7's walk-back needs one to resolve an earlier period |
| 23 | `snapshot_schema_version` ships from day one |
| 24 | One stateful crate; no use-case/SQL split, no repository seam |
| 25 | One mutable `PaySchedule` per Employer; changes refuse inside a finalized TaxYear |
| 26 | ~~`CompensationTerms` freeze once live finalized payroll references them~~ — **withdrawn** by 30 |
| 27 | `Employment` is never deleted; mis-created ones are voided |
| 28 | Actors are bare `TEXT`; no user model in this scope |
| 29 | **The re-read rule.** A fact later periods re-read freezes at finalization; a fact consumed once and frozen into the snapshot does not (ADR-0013) |
| 30 | `CompensationTerms` and `UnsupportedDeductionStatus` are correctable master data forever, under an audited reason, a named divergence list, and the guarantee that no finalized figure can move (§6.5) |
| 31 | `PriorEmployment` is Employment + TaxYear state; no row means `Unknown`; it freezes with the `OpeningBalance` |
| 32 | `UnsupportedDeductionStatus` is effective-dated on a pay-period start date; no row in force means `Unknown`; it does not freeze |
| 33 | Earnings are run-scoped; absence means no additional earnings, and that asymmetry with 31 and 32 is deliberate |
| 34 | `OpeningBalance` carries `first_salt_period_end` — the first period Salt is responsible for — under four guards, frozen with the figures (ADR-0014) |
| 35 | A Correction run holds exactly one Employment and auto-proposes nothing (ADR-0015) |
| 36 | A bare `Reversal` resolves its period; a `Reversal` is not a gap |
| 37 | An Ordinary run checks exactly one preceding period, by induction, and never crosses the TaxYear boundary |
| 38 | A Correction run checks its own period only — neither earlier nor later |
| 39 | Lineage may be null for a Correction that supersedes a reasoned removal or covers an omission; `correction_reason` is mandatory in every case |
| 40 | A Correction run's earnings are pre-populated from the reversed snapshot, degrading to empty when the snapshot schema is no longer readable |
| 41 | Editing a `Calculated` run's earnings or membership is accepted and reopens it as `Draft`; `Finalized` is the one absolute refusal |
