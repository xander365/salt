# Salt — Operator Identity, Authorization, HTTP API and First Web Payroll Slice

**Status:** Grilled and settled, 2026-09-01. Ready for `/to-spec`.  
**Purpose:** Settle the delivery boundary between Salt's stateful payroll application layer and its first browser client.  
**Next step:** three specs, in the order given in §0.3.  
**Not an implementation spec yet.**

ADRs written from this grill: [0016 opaque sessions](../adr/0016-opaque-server-side-sessions-not-jwt.md),
[0017 membership authorization, no RLS](../adr/0017-authorization-is-employer-membership-read-fresh-no-rls.md),
[0018 `salt-server` writes no SQL](../adr/0018-salt-server-writes-no-sql.md),
[0019 frozen actor text](../adr/0019-historical-actor-text-is-frozen.md).
Glossary terms added to `CONTEXT.md`: **Operator**, **EmployerMembership**.

---

# 0. Settled decisions

Sections 1–48 below are the grill's question set, kept as the record of what was
asked. This section is the answer set. Where the two disagree, this section wins.

## Product and process

**0.1 Deployment model.** Hosted service, multi-Employer safe from the first
migration, but no public signup. Operators are created deliberately.

**0.2 First Owner and first Employer.**

```text
salt-server bootstrap \
    --email <operator email> \
    --display-name <operator display name> \
    --employer-name <employer name> \
    --period-end-day <1..28 | last-day-of-month>
```

It creates the Operator,
the Employer and the Owner membership in one transaction, and refuses if any
Operator already exists. The password is read from stdin, never a flag.

The review found the Employer half incomplete: the `employer` table carries a
PaySchedule and **no name**, so `--employer-name` had nowhere to go, and a
PaySchedule is required and must not be guessed. Both are settled here.

*Name.* `employer` gains one column, `name TEXT NOT NULL`, non-blank by CHECK like
every other attribution column in the schema. One human-readable field, because the
web app must title a page and a list row with something a person recognises. It is
**mutable**, and the capability that will change it is Owner's Employer
configuration — which is deferred (§0.39), so for Specs 1–3 it is set once at
bootstrap and never edited. Nothing about payroll reads it; a FinalizedPayroll
explains itself and does not look back at a name.

*PaySchedule.* Never defaulted. `bootstrap` takes an explicit
`--period-end-day <1..28|last-day-of-month>` and **refuses without it**, printing
what the options mean. "Last day of month" is the common answer and is still typed
deliberately, because a wrong pay schedule is not a display bug: ADR-0005 makes it
the thing every period boundary is generated from, and it cannot be changed
mid-tax-year.

**0.3 Delivery order — three specs, not one.** This clause is authoritative. Any
sentence elsewhere in this document calling for a single implementation spec is
stale and superseded. The three are not merged, and each is shippable and
independently testable.

> **SPEC 1 — Identity and the authorized request.**
> Operator, EmployerMembership, opaque sessions, the bootstrap command, the
> runtime-database seam (§0.17), and `AuthorizedEmployerContext`. Provable with
> zero payroll routes.
>
> **SPEC 2 — The HTTP surface.**
> `payroll-app` read models (§0.30, §0.31), HTTP DTOs, the error contract, the
> route table (§0.22), and the ordinary-payroll HTTP tracer bullet.
>
> **SPEC 3 — The browser.**
> The React/Vite application and the single Playwright tracer bullet.

## Identity and authorization

**0.4 Terminology.** **Operator**. `User` and `Account` are already spent in
`CONTEXT.md` — "user" is warned off under Person, "account" under Employer.

**0.5 Identity shape.** One global Operator identity gaining Employers through
**EmployerMembership**. Two roles: `Owner` and `PayrollOperator`. An Operator is
global; a **Person is Employer-scoped** (§0.38, ADR-0020) — the two identities are
deliberately different shapes, because an Operator signs in and a Person does not.

**0.6 Role capabilities.** `Owner` is required only for membership administration
and Employer configuration (create/remove membership, change pay schedule, create
Employer). Everything payroll — employments, compensation terms, declarations,
runs, calculate, finalize — is `PayrollOperator` or above. Reads are open to both.
A route not on the Owner list checks no role at all. **The Owner-only capabilities
have no route in Specs 1–3** — see §0.39.

**0.7 The authorization seam.** An Axum extractor yields
`AuthorizedEmployerContext { operator_id, employer_id, role }`. Handlers take the
EmployerId **out of that struct**, never from the path, so a handler that skips
authorization does not compile. See ADR-0017.

**0.8 Freshness.** One joined query per request — session, Operator status,
membership — with no caching. Revocation therefore bites on the next request.

**0.9 No RLS.** Application authorization plus an explicit `employer_id` filter in
every `payroll-app` read is the second layer. See ADR-0017 for why RLS is not.

**0.10 404 vs 403.** Anything outside the caller's authorized Employers is **404**.
**403** is only for a member whose role is too low.

**0.11 Actor identity.** `salt-server` passes `operator:<OperatorId>`, derived from
the session. Historical actor text is frozen and never becomes a foreign key. See
ADR-0019.

## Authentication

**0.12 Mechanism.** Opaque server-side sessions in PostgreSQL,
`HttpOnly; Secure; SameSite=Lax; Path=/`, no `Domain`. See ADR-0016. Many sessions
per Operator, logout deletes the row, token rotates on login.

*Token.* At least **256 bits** from the operating system's cryptographic random
source, URL-safe base64 for the cookie value. Only a **SHA-256 hash** of it is
stored, and lookup is by that hash. Not a password hash: the token is already
high-entropy, so a slow KDF buys nothing and would cost a hash on every request.
The plaintext token exists in the response that mints it and nowhere else — never
in a log line.

*Timers.* The row carries `created_at`, `last_seen_at` and `expires_at`. A session
is valid when `now < created_at + 12h` (absolute) **and**
`now < last_seen_at + 8h` (idle). Both are checked in the same query that loads
the session, so an expired row is never treated as valid even before it is deleted.

*Extending the idle window.* An authenticated request updates `last_seen_at` — but
**throttled: only when it is more than 5 minutes stale**. A page that fires six
queries must not cost six writes. Five minutes of drift against an eight-hour
window is not a security property, and the write happens outside the request's own
transaction so it can never roll a handler back.

*Expired rows.* **Lazy deletion.** A lookup that finds an expired row deletes it and
answers 401; login deletes that Operator's other expired rows. No sweeper process,
no cron. Validity is decided by the timestamps, never by the row's existence, so a
row that outlives its expiry authorizes nothing.

**0.12a Account lockout.** Per account, in PostgreSQL, with the counters on the
`operator` row: **10 failed attempts within 15 minutes locks the account for 15
minutes**, after which it unlocks by itself with no administrator in the loop. A
successful login resets the counter to zero. Ten is chosen to be far above human
mistyping and far below useful guessing. A locked account returns the same 401
`invalid_credentials` as any other failure — saying "locked" would confirm the
email exists.

An **unknown** email does the same Argon2id work against a fixed dummy verifier, so
the response time does not separate "no such account" from "wrong password".
Unknown emails have no row and therefore no counter; Caddy's per-IP limit is what
answers a flood of them, and this is the boundary between the two mechanisms.

**0.13 Passwords.** Argon2id via the `argon2` crate. Failed login always returns the
same 401 `invalid_credentials` and spends the same time, hashing against a dummy
verifier when the account does not exist. Per-account lockout lives in PostgreSQL;
per-IP throttling lives in Caddy. Password reset is out of this slice.

**0.14 CSRF.** `SameSite=Lax` plus a required `X-Salt-Request: 1` header on every
mutating route. No token table: a browser cannot set a custom header cross-origin
without a preflight, and no cross-origin preflight is allowed.

**0.15 CORS.** None, in either environment. Production is one origin behind Caddy;
development is one origin because Vite proxies `/api` to the server.

## Server and process

**0.16 Crate.** `crates/salt-server`, library plus thin binary. The library exposes
the router so HTTP tests build the real thing in-process. It does not depend on
`sqlx` and writes no SQL — see ADR-0018, which also settles that the `operator`,
`employer_membership` and `session` tables live in `payroll-app`'s schema.

**0.17 The runtime database seam.** ADR-0018 said what `salt-server` may not do; it
did not say how the server then owns a pool. Today every use case takes
`pool: &PgPool`, so naming one would itself require the `sqlx` dependency the ADR
forbids. Settled: `payroll-app` exports one opaque handle and one constructor, and
every existing use-case signature changes from `&PgPool` to `&SaltDatabase`.

```text
payroll_app::SaltDatabase          opaque; wraps a PgPool with no public accessor,
                                   no Deref, no Into
payroll_app::SaltDatabase::connect(&DatabaseConfig)
                                   builds the pool, installs the after-connect
                                   hook, verifies the schema version, or refuses
payroll_app::SaltDatabase::from_pool(PgPool)
                                   test-only in practice: constructing the argument
                                   needs sqlx, which salt-server does not have
```

`connect` does the three things §0.17's startup sequence needs, all inside the crate
that owns the schema:

1. **Pool construction.** Size, timeouts and connect options come from a
   `DatabaseConfig` struct of plain values — url, max connections, timeouts —
   that `salt-server` fills from the environment. No `sqlx` type appears in it.
2. **`SET ROLE payroll_app` per connection.** Installed as an `after_connect` hook
   on the pool, so it holds for every checkout including ones a future pool
   refill creates. Not a per-call responsibility, and not something a caller can
   forget.
3. **Schema version check.** Compares the applied migration version against the
   version compiled into `payroll-app` and returns a refusal if the database is
   behind. The server turns that into a refusal to start.

This is a pool type and a constructor, not a `Database` trait or a repository
seam — ADR-0009 refused a seam with one implementation and that refusal stands.
The opacity is what makes ADR-0018 structural rather than a convention: a
`SaltDatabase` cannot be unwrapped, and the escape hatch `from_pool` needs a
`PgPool` value, which a crate without `sqlx` cannot produce.

**0.17a Startup.** Deploy runs migrations with the admin credential. The server calls
`SaltDatabase::connect`, and a schema-behind refusal from it aborts startup — loud
and early. The runtime credential is a login role granted `payroll_app`; the
migration credential is absent from the server's environment entirely.

**0.18 Config.** Environment variables only, read once at startup into one struct
that refuses to build on a missing or malformed value, with secrets redacted in its
`Debug`. `GET /api/health` (liveness) and `GET /api/ready` (a real database round
trip), both unauthenticated.

**0.19 Headers and limits.** Caddy owns TLS, HSTS and per-IP rate limiting. Axum
owns the 256 KB JSON body limit, CSP, `X-Content-Type-Options`, frame-deny, and a
per-request id that appears in every log line and in `details.requestId` on 500s
only. Anything an application-level test should assert belongs in Axum.

## API

**0.20 Topology.** Caddy serves the built React bundle at `/` and reverse-proxies
`/api/*` to `salt-server`. Axum never learns about static files.

**0.21 No versioning.** The SPA and the server ship from the same release. A version
segment that never changes is a lie. `/v2` arrives with the first client Salt does
not deploy.

**0.22 Route table.** Every Employer-scoped URL carries the EmployerId, including
nested resources, so the extractor has one on every protected route without
exception.

```text
POST   /api/session                      login
DELETE /api/session                      logout
GET    /api/session                      who am I

GET    /api/employers
GET    /api/employers/{e}/employments
POST   /api/employers/{e}/employments      personId or fullName, exactly one
GET    /api/employers/{e}/employments/{em}
POST   /api/employers/{e}/employments/{em}/compensation-terms
POST   /api/employers/{e}/employments/{em}/prior-employment
POST   /api/employers/{e}/employments/{em}/unsupported-deductions
POST   /api/employers/{e}/employments/{em}/opening-balance

GET    /api/employers/{e}/payroll-runs
POST   /api/employers/{e}/payroll-runs
GET    /api/employers/{e}/payroll-runs/{r}
PUT    /api/employers/{e}/payroll-runs/{r}/members/{em}/earnings
POST   /api/employers/{e}/payroll-runs/{r}/calculate
POST   /api/employers/{e}/payroll-runs/{r}/finalize

GET    /api/employers/{e}/finalized-payroll/{f}
GET    /api/employers/{e}/finalized-payroll/{f}/traces
```

Nothing else. No Employer creation route, no membership routes (§0.39), no Person
routes (§0.38), no reversal, no correction, no ActionLog.
Earnings is `PUT` because `set_run_earnings` replaces the whole list, and calling
that a `POST` would hide the fact that it is idempotent.

**0.23 Error envelope.** Salt's own `{ "error": { code, message, details } }`, not
RFC 9457 — which wants a URI per error type and has nowhere natural for structured
`details` such as period lists and mismatch fields. Every `PayrollAppError` and
`PayrollError` variant maps to exactly one stable `code` in one file in
`salt-server`, kept exhaustive by a `match`. No SQL text, no stack traces.

**0.24 Status codes.** 401 unauthenticated; 403 role too low; 404 outside the
authorized scope; 409 lifecycle, mismatch and acknowledgement conflicts; 422
semantic payroll refusal; 400 malformed; 500 with a request id.

**0.25 Calculate is not an error.** A partial calculation is a normal outcome:
`POST …/calculate` returns **200** with the run detail, each member carrying its own
`refusal`, and the run's own `status` telling React whether Finalize is allowed.

**0.26 Finalization mismatch.** 409, one code per variant —
`finalization_input_mismatch`, `finalization_rules_mismatch`,
`finalization_calculation_mismatch` — with the differing employment ids in
`details`. React's recovery is "facts changed since you calculated; recalculate".

**0.27 Divergence acknowledgement.** 409 `master_data_divergence_not_acknowledged`
with `details.divergingPeriods` as real dates, not an opaque token: the human must
read which months are affected. Resubmission is the same command with the list
filled in, and `payroll-app` recomputes the list inside its own transaction, so a
stale acknowledgement is refused with the new list. No `force` flag will exist.

**0.28 Retry after a lost response.** A retried finalize meets a run already
`Finalized` and gets 409 `payroll_run_already_finalized` with
`details.finalizedPayrollId`. React reads that code and navigates to the finalized
view — the user sees success, because it was success. No idempotency keys: database
uniqueness already makes a duplicate impossible, so what is owed is UX, not a
mechanism.

**0.29 Sensitive responses.** Hand-written finalized-payroll view DTOs carrying the
nine figures, the period, the pay date and the SaltVersion. PAYE and SSC traces are
a **separate endpoint**. Raw snapshot JSON is never a response body — the moment it
is, its internal shape becomes a public contract.

## Read models

**0.30 The surface `payroll-app` gains.** No pagination, no filters yet. Every one
takes `employer_id` explicitly and filters on it in SQL.

```text
ListEmployers(operator_id)
ListEmployments(employer_id)
GetEmploymentDetail(employer_id, employment_id)
ListPayrollRuns(employer_id)
GetPayrollRunDetail(employer_id, run_id)
GetFinalizedPayrollDetail(employer_id, finalized_payroll_id)
```

**0.31 Failed calculation after refresh — the answer to §31 and §44 J.** Nothing is
persisted and nothing is recomputed by the calculator. `GetPayrollRunDetail` reports
per member a `blockers: [{ code, details }]` list, computed by **reading standing
facts**, under the same stable codes as the error contract. Empty means ready.

The five states it must distinguish:

```text
prior_employment_unknown                  no declaration in force
prior_employment_treatment_unconfirmed    declared with figures; treatment is
                                          unconfirmed, so it is refused too
unsupported_deduction_status_unknown      no declaration in force
unsupported_deductions_present            declared present; details.kinds names them
no_compensation_terms_in_force            no CompensationTerms row covers the
                                          period end
```

The two "present" states matter as much as the two "unknown" ones: `CONTEXT.md`
records that known PriorEmployment figures are refused while their treatment is
unconfirmed, and that a present UnsupportedDeductionStatus is refused as firmly as
an unknown one. A UI that showed only the unknowns would tell an Operator they were
ready when they were not. `details.kinds` is carried for the present case because
"you have unsupported deductions" without naming them is unactionable.

Three limits, stated so `/to-spec` does not widen them:

1. **`blockers` is not a persisted copy of the last Calculate refusal.** Nothing is
   stored. It is derived on every read from the facts as they stand now, so
   clearing a blocker clears it from the page.
2. **It does not promise to reproduce every `PayrollError`.** It covers exactly the
   states above — the ones knowable from standing facts. A refusal that only
   appears once arithmetic runs is out of its reach by construction.
3. **A GET never runs the calculator.** If Calculate previously failed for a reason
   not in the list, refresh shows the run as `Draft` with no blocker naming it, and
   the Operator recalculates to reproduce the refusal. That is the honest answer:
   the alternative is either a stale stored opinion or arithmetic behind a GET, and
   both were rejected.

This is not duplicated logic — the crate that refuses is the crate that reports
readiness, and "no declaration row exists" is a fact lookup, not a payroll rule.

## React

**0.32 Stack.** React + TypeScript + Vite + React Router + TanStack Query. No SSR:
there is no public page and no SEO surface. API types are hand-written in one
`api/types.ts`; code generation is revisited when the surface stops fitting in one
file.

**0.33 Auth state.** React holds none. `GET /api/session` is the truth, returning the
Operator and their memberships or 401.

**0.34 Employer switching.** One membership means redirect straight in, no switcher.
Several means a plain list page at `/app`. The URL carries the EmployerId either way,
so adding a switcher later changes nothing behind it.

**0.35 First UI scope.** The minimum standing-data screens that make one Employment
payable — Employment, CompensationTerms, PriorEmployment,
UnsupportedDeductionStatus and **OpeningBalance** — then the ordinary payroll flow.
The Employment form takes the person's name directly (§0.38); there is no separate
Person screen, and an Operator never types a `PersonId`.
OpeningBalance is in because the first real customer will be adopting Salt mid-year,
and without it their first Calculate refuses with a blocker no screen can clear.
Reversal, correction and ActionLog screens stay out.

## Tests

**0.36 HTTP.** `#[sqlx::test]` hands the test a `PgPool`, which it wraps with
`SaltDatabase::from_pool` — the one place that constructor is used, and a place that
already depends on `sqlx` (§0.17). Plus the real router in-process via
`tower::ServiceExt::oneshot` — no port, no browser. These own the eight proofs in
§33, in particular "Employer A cannot fetch Employer B's run by known id" and
"ActionLog actor comes from the session, not the body".

**0.37 Browser.** One Playwright test for the whole journey. One, not a suite.

## Closed after review

**0.38 Person — the minimum model.** `employment.person_id` is a bare `TEXT` column
today with no table behind it, so an Operator would be asked to type opaque ids.
A `person` table is added, and no more of one than this slice needs.

*Ownership and scope.* A Person is **Employer-scoped**: `person` carries
`employer_id`, and the same human employed by two Employers is two Person rows.
This looks wrong beside `CONTEXT.md`, which defines Person as "a human being,
independent of any job they hold" — and it is a deliberate narrowing, recorded here
so nobody thinks it was missed. Salt has no way to know two rows are the same human
(no national-id matching, no identity resolution, and no consent story for sharing
one Employer's personal data with another). A global Person would be a claim Salt
cannot substantiate, and the claim it would make wrongly — this Employer may see
that this human works elsewhere — is exactly the isolation §13 exists to defend.
Employer-scoped Persons are also what makes §0.30's "every read filters on
`employer_id`" total, with no exception carved out for people.

*Fields.* `id`, `employer_id`, `full_name`, `created_at`, `created_by`. One name
field, not given/family, because Salt does not yet render anything that needs the
halves separately and splitting names correctly across cultures is a real problem
not worth solving speculatively. **No** date of birth, tax number, address, bank
details or contact — payroll may want them one day, and "one day" is not a reason.

*Id.* `payroll-app` mints the `PersonId` (UUIDv7, like every other id it owns),
the same way it already mints `EmployerId` and `EmploymentId`. The pure crate's
`PersonId` type is unchanged; only the caller of `PersonId::new` moves.

*Creation.* `POST /api/employers/{e}/employments` takes `personId` **or**
`fullName`, exactly one. Given a name it creates the Person and the Employment in
one transaction; given an id it verifies the Person belongs to the authorized
Employer and refuses with 404 otherwise. Two calls and a client-side join would let
a browser abandon a Person with no Employment, which is a row nobody will ever
clean up. There is no separate Person creation route and no Person list route in
Specs 1–3.

*Display.* `ListEmployments` returns `fullName` beside the ids; `GetEmploymentDetail`
returns it too. Every screen an Operator reads names a human, never only a
`PersonId`. This is the whole point of the clause.

**0.39 Membership administration is deferred.** §0.6 names Owner capabilities that
§0.22 exposes no routes for. That gap is deliberate and is now stated: **for Specs
1–3, Employer creation after bootstrap and membership administration have no HTTP
route and no UI.**

- Both roles are **modelled now** — the `role` column exists and every route
  enforces §0.6 — because retrofitting a role column onto live memberships is far
  more expensive than carrying an enum with one route-relevant distinction.
- **Bootstrap creates the first Owner membership** (§0.2), and that is the only way
  a membership comes into existence in these three specs.
- **Tests construct further memberships through `payroll-app` use cases**, not
  through HTTP. The `PayrollOperator`-is-refused test and the non-member-gets-404
  test both need a second Operator, and both set it up in Rust.
- `/to-spec` must not infer `POST /api/employers`, membership routes, or an
  invitation flow from §0.6. If a route is not in §0.22, it does not exist.

**0.40 Which spec owns which schema change.** Three migrations fall out of the
clauses above, and each belongs to exactly one spec so neither `/to-spec` invocation
writes the other's.

```text
SPEC 1   operator, employer_membership, session tables
SPEC 1   employer gains name TEXT NOT NULL, non-blank by CHECK   (§0.2)
SPEC 1   every use case moves from &PgPool to &SaltDatabase       (§0.17)
SPEC 2   person table, and employment.person_id gains its
         foreign key to it                                        (§0.38)
SPEC 3   none
```

The `&SaltDatabase` change is not a migration but belongs in SPEC 1 for the same
reason: it touches every existing use case once, and doing it twice would be worse
than doing it early.

---

## 1. Why this document exists

Salt now has two strong internal layers:

```text
crates/payroll
    pure payroll calculation domain
        ↑
crates/payroll-app
    PostgreSQL-backed application use cases
```

The second layer can already create Employers and Employments, record payroll facts, create/calculate/finalize payroll runs, rebuild YTD, reverse finalized payroll, create CorrectionRuns, correct master data while naming divergence, preserve immutable history, and write ActionLog entries.

It deliberately does **not** yet own:

- an HTTP server;
- connection-pool construction;
- operator/user accounts;
- authentication;
- authorization;
- browser sessions;
- API DTOs;
- HTTP error mapping;
- React;
- browser state;
- UI navigation;
- payslip rendering.

That separation was intentional.

The goal of this design is:

> A real human operator can sign into Salt, act only on an Employer they are authorized to operate, run one ordinary payroll through HTTP and a minimal React UI, and never bypass the existing `payroll-app` use cases.

This is the first delivery-layer design. It is not yet "design the whole Salt UI".

---

# 2. Existing decisions this grill must NOT reopen casually

## 2.1 `payroll` remains pure

The pure crate stays free of SQLx, Axum, HTTP, authentication, sessions, JWT libraries, async framework concerns and frontend concerns.

Nothing in this design may move transport or identity logic into `payroll`.

## 2.2 `payroll-app` remains the stateful application boundary

`payroll-app` owns application use cases, SQLx/PostgreSQL transactions, payroll persistence, finalization, reversal, correction, YTD reconstruction and ActionLog semantics.

An HTTP route may:

```text
authenticate
authorize
parse request
validate transport-level shape
call payroll-app
map result/refusal to HTTP
```

It must not:

```text
recalculate PAYE
rebuild YTD itself
decide correction lineage
write payroll tables directly
duplicate sequencing checks
invent its own finalization rules
```

## 2.3 `payroll-app` currently accepts actor identifiers as plain text

The persistence design deliberately deferred identity.

This next design must give that actor a trustworthy source.

HTTP clients must never be allowed to submit an actor identity and have Salt believe it.

The authenticated server determines the actor.

## 2.4 Employer access is a security boundary

`EmployerId` is not authorization.

> Every Employer-scoped command or query runs only after Salt proves that the authenticated operator is allowed to act on that Employer.

This must be difficult to forget.

## 2.5 Finalization remains the deliberate payroll approval

There is no `Reviewed` workflow state.

The UI may show confirmation screens and warnings, but the persisted payroll lifecycle stays:

```text
Draft → Calculated → Finalized
```

## 2.6 Warnings and acknowledgements are real outputs

Structured refusals such as divergence acknowledgements must cross HTTP intact enough for the browser to continue safely.

Do not flatten them to `"Bad request"` or force React to parse error strings.

---

# 3. Scope

Settle enough identity, authorization, API and browser behaviour to support:

```text
Operator signs in
    ↓
authorized Employer selected/resolved
    ↓
view Employments
    ↓
create Ordinary PayrollRun
    ↓
view proposed run membership
    ↓
set any taxable allowance Earnings
    ↓
calculate
    ↓
view PayrollCalculation per Employment
    ↓
finalize deliberately
    ↓
view immutable finalized result
```

This should prove:

1. authentication works;
2. Employer isolation works;
3. the server owns the database pool;
4. HTTP uses existing `payroll-app` use cases;
5. typed refusals survive the HTTP boundary;
6. the browser can complete a real ordinary payroll;
7. no frontend code owns payroll arithmetic or payroll invariants.

---

# 4. Explicitly out of scope

Do not expand this design into:

- Tauri;
- mobile;
- public third-party API access;
- SSO;
- enterprise RBAC;
- per-field permissions;
- refresh-token ecosystems;
- API keys;
- service-to-service auth;
- microservices;
- GraphQL;
- WebSockets;
- event buses;
- background jobs;
- notifications;
- email delivery;
- payslip PDFs;
- employee self-service;
- ETX;
- SSC filing;
- tax-period reporting;
- accounting export;
- leave;
- overtime;
- bonuses;
- loans;
- voluntary deductions;
- remuneration classification;
- resolving SC-OPEN-1 through SC-OPEN-5;
- a general design system;
- offline mode;
- optimistic multi-user payroll editing;
- a generic framework over Axum.

---

# 5. Proposed architecture

```text
┌──────────────────────────────────────────────┐
│ React browser app                            │
│ pages / forms / queries                      │
│ no database                                  │
│ no payroll arithmetic                        │
└───────────────────────────┬──────────────────┘
                            │ HTTPS + JSON
                            ▼
┌──────────────────────────────────────────────┐
│ salt-server                                  │
│ Axum                                         │
│ pool ownership                               │
│ authentication                               │
│ authorization                                │
│ session handling                             │
│ HTTP DTOs                                    │
│ HTTP error mapping                           │
│ tracing / startup / shutdown                 │
│ NO payroll-rule duplication                  │
└───────────────────────────┬──────────────────┘
                            │
                            ▼
┌──────────────────────────────────────────────┐
│ payroll-app                                  │
│ concrete use cases                           │
│ SQLx / PostgreSQL                            │
└───────────────────────────┬──────────────────┘
                            │
                            ▼
┌──────────────────────────────────────────────┐
│ payroll                                      │
│ pure calculation domain                      │
└──────────────────────────────────────────────┘
```

Strong starting position:

> Add a new `crates/salt-server` crate depending on `payroll-app`. Neither existing crate depends on it.

**GRILL:** binary-only vs lib+bin, router construction, dependency injection of pool/config, and whether the crate split earns its existence without multiplying seams.

---

# 6. Operator identity

Candidate:

```text
Operator
- OperatorId
- email / login identifier
- password credential metadata
- display_name
- status
- created_at
```

Potential statuses:

```text
Active
Disabled
```

Questions:

1. Is the product term `Operator`, `User`, or `Account`?
2. Is login email-based?
3. Are emails unique case-insensitively?
4. Does one global identity gain Employer memberships?
5. Can one identity belong to multiple Employers?
6. How is an Operator disabled without deleting history?
7. What should ActionLog actor values contain going forward?
8. Do existing actor text columns become foreign keys now, later, or never?

Strong candidate:

> Persist a global OperatorId. Do not rewrite immutable historical actor text just to normalize identity.

---

# 7. Employer membership and authorization

Candidate:

```text
EmployerMembership
- OperatorId
- EmployerId
- role
- status
```

Possible v1 roles:

### Option A

```text
Owner
```

Anyone with membership has full access.

### Option B

```text
Owner
PayrollOperator
```

Possible semantics:

```text
Owner:
- employer configuration
- membership/security administration
- payroll operations

PayrollOperator:
- employment/payroll operations
- no membership/security administration
```

Do not invent five roles without a use case.

Questions:

1. How many roles exist in v1?
2. Can an Operator belong to multiple Employers?
3. Is an "active Employer" a UI concept only?
4. Is membership checked on every Employer-scoped request?
5. How is authorization expressed once in Axum instead of hand-coded repeatedly?
6. What happens to an open session when membership is revoked?
7. Does authorization read membership fresh on every request?
8. Is there any system-level super-admin in v1?

Strong candidate:

> No normal API super-admin. Database administration is not an application role.

---

# 8. Authentication — do not assume JWT

Candidate starting position:

> Opaque server-side session identified by a secure HttpOnly cookie.

```text
login credentials
       ↓
verify password
       ↓
random session token
       ↓
session stored server-side
       ↓
Secure + HttpOnly cookie
```

Advantages for the first web app:

- easy logout/revocation;
- disabled users can stop immediately;
- membership changes need no token expiry dance;
- no refresh token;
- browser JavaScript never reads the credential.

JWT may still be right. The grill must compare rather than assume.

Questions:

1. Opaque session or JWT?
2. If JWT, what current Salt problem needs self-contained bearer tokens?
3. Where would the browser store it?
4. How would logout/revocation work?
5. How quickly do role changes take effect?
6. Does future Tauri actually require the same auth mechanism?
7. Is using one mechanism for web now and another client mechanism later acceptable?
8. Is horizontal scale imminent enough to make local/session storage a blocker?
9. If sessions are in PostgreSQL, what are expiry and cleanup semantics?

Do not choose JWT because "APIs use JWT".

---

# 9. Password handling

If Salt owns credentials:

- never store passwords;
- use a modern password-hashing algorithm/library;
- store only verifier/hash and parameters;
- never log passwords;
- enforce sensible length limits;
- credentials never travel in URLs;
- login responses should not unnecessarily reveal account existence.

Questions:

1. Does Salt own credentials or outsource identity?
2. Which Rust library/algorithm?
3. Is password reset in this slice?
4. How is the first Owner created?
5. Is a bootstrap CLI/admin command enough initially?
6. Is email verification required before the first payroll tracer bullet?

Strong starting position:

> Bootstrap the first Owner explicitly. Do not build public signup, invitation mail and reset mail just to reach the first usable payroll.

---

# 10. Sessions

Candidate:

```text
Session
- SessionId / token hash
- OperatorId
- created_at
- expires_at
- revoked_at? or delete-on-revoke
```

Questions:

- absolute expiry?
- idle expiry?
- many sessions per Operator?
- delete or revoke?
- rotate after login?
- rotate after privilege change?
- cookie Path/Domain?
- development HTTP behaviour?
- cleanup strategy?

Avoid browser fingerprinting.

---

# 11. CSRF, CORS and deployment topology

Candidate production topology:

```text
https://salt.example/
    React

https://salt.example/api/
    Axum
```

Same origin.

Advantages:

- simple cookies;
- simple CORS story;
- simpler CSRF model;
- one public origin.

Questions:

1. Same-origin production?
2. Caddy serves frontend or Axum?
3. What CSRF defense is used for state-changing cookie-authenticated requests?
4. Is SameSite alone enough for the chosen flows?
5. Is an anti-CSRF header/token required?
6. Is production CORS disabled entirely?
7. How does Vite dev on another port work?

Strong candidate:

> Same-origin production. Explicit development CORS only. Never wildcard origin with credentials.

---

# 12. Authenticated and authorized request context

Candidate:

```text
Cookie
  ↓
AuthenticatedOperator extractor
  ↓
Employer route parameter
  ↓
AuthorizedEmployer extractor/guard
  ↓
AuthorizedEmployerContext {
    operator_id
    employer_id
    role
}
  ↓
handler
  ↓
payroll-app
```

Questions:

- extractor or middleware?
- how many DB reads per request?
- 401 vs 403?
- scoped 404 for resource enumeration?
- how do nested resources prove Employer ownership?

This is load-bearing.

---

# 13. Tenant isolation

Principle:

> A caller never receives a PayrollRun, Employment, FinalizedPayroll, ActionLog entry or correction merely because they know its id.

Candidate HTTP URLs may include EmployerId:

```text
GET /api/employers/{employer_id}/payroll-runs/{run_id}
```

but nesting alone is not authorization.

Questions:

1. Should every Employer resource URL include EmployerId?
2. Should `payroll-app` query functions take EmployerId explicitly?
3. Are existing write use cases safe after server pre-authorization?
4. Should PostgreSQL Row Level Security be added?
5. Does RLS uniquely improve the threat model, or duplicate application authorization?

Strong starting position:

> No RLS initially unless the grill identifies a concrete second database access path or threat it uniquely closes.

**GRILL THIS HARD because payroll data is sensitive.**

---

# 14. Browser read/query surface

The application layer is command-heavy. React needs purpose-built reads.

Candidate additions to `payroll-app`:

```text
ListEmployments(employer_id)
GetEmploymentDetail(employer_id, employment_id)

ListPayrollRuns(employer_id, filters)
GetPayrollRunDetail(employer_id, payroll_run_id)

GetWorkingPayrollResults(employer_id, payroll_run_id)

GetFinalizedPayrollDetail(
    employer_id,
    finalized_payroll_id
)

ListFinalizedPayrollForEmployment(
    employer_id,
    employment_id
)

ListActionLog(employer_id, filters)
```

These may return application read models such as:

```text
EmploymentListItem
PayrollRunSummary
PayrollRunDetail
PayrollRunMemberDetail
FinalizedPayrollDetail
ActionLogItem
```

Questions:

1. Which exact reads are needed for the first ordinary payroll UI?
2. Do DB/query semantics remain in `payroll-app`? Strong candidate: yes.
3. Does `salt-server` translate application read models to HTTP DTOs?
4. Pagination from day one?
5. Which filters are actually needed?

No generic query repository.

---

# 15. API command style

Illustrative routes:

```text
POST /session/login
DELETE /session

GET  /employers
GET  /employers/{employer_id}/employments

POST /employers/{employer_id}/payroll-runs
GET  /employers/{employer_id}/payroll-runs/{run_id}

POST /payroll-runs/{run_id}/calculate
POST /payroll-runs/{run_id}/finalize
```

Do not pretend finalization is:

```text
PATCH { "status": "Finalized" }
```

Clients are not allowed to assign lifecycle states.

**GRILL:** route shape, resource nesting, command verbs, and whether API versioning is needed now.

---

# 16. API versioning

Do we need `/api/v1`?

Question:

> Is the React frontend deployed lockstep with the backend, or are independent/public clients expected soon?

If only lockstep internal web exists, versioning may be premature.

If future Tauri/public clients will lag releases, a version namespace may be cheap insurance.

Settle explicitly.

---

# 17. HTTP DTO boundary

Do not blindly expose domain structs.

Candidate:

```text
HTTP request DTO
    ↓
conversion
    ↓
payroll-app/domain arguments

payroll-app result
    ↓
conversion
    ↓
HTTP response DTO
```

Reasons:

- avoid making internal enum serialization a permanent public contract;
- avoid exposing raw snapshot data;
- prevent infrastructure errors from leaking;
- allow UI-oriented response shapes.

But do not create pointless one-to-one DTOs for every tiny value.

Grill the boundary, not DTO quantity.

---

# 18. HTTP error contract

Broad categories:

```text
Authentication
Authorization
Application/domain refusal
Infrastructure/internal failure
```

Candidate status guidance:

```text
401 unauthenticated
403 forbidden
404 absent in authorized scope
409 state/lifecycle/concurrency conflict
422 semantic payroll refusal
400 malformed request
500 unexpected infrastructure failure
```

Candidate body:

```json
{
  "error": {
    "code": "prior_employment_unknown",
    "message": "Prior employment has not been established.",
    "details": {
      "employmentId": "...",
      "taxYear": "..."
    }
  }
}
```

Requirements:

- stable machine-readable code;
- human-readable message may evolve;
- structured actionable details;
- never parse strings in React;
- no SQL leakage;
- no stack traces;
- request/correlation id on unexpected failures.

Questions:

1. RFC 9457 Problem Details or custom Salt envelope?
2. How are nested PayrollErrors represented?
3. Validation error shape?
4. Can calculate return multiple member refusals?
5. Which errors are retryable?
6. How are three-way finalization mismatches represented?

---

# 19. Warnings and two-step acknowledgements

Existing divergence protocol is roughly:

```text
correction request with acknowledged_periods = []
        ↓
MasterDataDivergenceNotAcknowledged {
    periods: [...]
}
        ↓
UI shows exact periods
        ↓
same command resubmitted with exact acknowledgement
        ↓
success only if current list still matches
```

HTTP must preserve it.

Do **not** replace with:

```json
{ "force": true }
```

Questions:

- 409 or 422?
- raw period list or opaque challenge token?
- what happens if divergence changes between requests?
- how does React present the acknowledgement?

---

# 20. Idempotency and retries

Consider:

```text
CreateOrdinaryPayrollRun
FinalizePayrollRun
ReverseFinalizedPayroll
CreateCorrectionRun
```

Scenarios include double-clicks and lost responses.

Questions:

1. If finalization committed but the response was lost, what does retry return?
2. Conflict or already-finalized success representation?
3. Does create-run need an idempotency key?
4. Is browser button disabling plus database invariants enough initially?
5. Which commands are safe to retry?

"Duplicate history impossible" is not the same thing as good retry UX.

---

# 21. Server process ownership

Candidate `salt-server` owns:

```text
configuration
PostgreSQL pool
Axum Router
TCP listener
shutdown handling
tracing/logging
```

Possible startup:

```text
load config
  ↓
validate config
  ↓
create pool
  ↓
verify database/schema
  ↓
build Router
  ↓
listen
```

Questions:

- auto-run migrations at startup or deploy separately?
- refuse startup when schema is behind?
- env-only config?
- secret handling?
- health endpoint?
- readiness endpoint?

Strong starting position:

> Deploy runs migrations explicitly. Runtime server does not silently mutate schema on every boot.

---

# 22. Observability

Candidate minimum:

- `tracing`;
- structured logs;
- request id;
- latency;
- HTTP status;
- OperatorId/EmployerId where safe;
- use-case name;
- internal infrastructure details only in server logs.

Never log:

- passwords;
- session tokens;
- full cookies;
- complete payroll snapshots by default;
- sensitive employee values merely because debug is enabled.

ActionLog and operational tracing remain separate concepts.

---

# 23. Sensitive payroll responses

Do not expose raw frozen JSON just because it exists.

The UI may need:

```text
Basic Pay
Taxable Allowances
Gross
Taxable Remuneration
PAYE
Employee SSC
Employer SSC
Net
PAYE/SSC traces
```

Questions:

1. Which snapshot fields are exposed?
2. How are older snapshot versions rendered?
3. Should PAYE/SSC traces show by default or under detail?
4. Are ActionLog contexts safe to expose wholesale?
5. How do salary before/after values affect authorization?

Strong candidate:

> Return intentional finalized-payroll view DTOs, not raw snapshot columns.

---

# 24. React responsibilities

React owns:

```text
routing
forms
display
server-state queries
mutation lifecycle
warnings/confirmations
session UX
```

It does **not** own:

```text
PAYE math
SSC math
PeriodsElapsed
Resolved semantics
finalization legality
correction legality
```

Frontend validation can improve UX but never replaces server enforcement.

---

# 25. React technology choices

Only grill choices that affect architecture.

Candidate:

```text
React + TypeScript
Vite
React Router
TanStack Query
```

Questions:

1. Vite SPA or Next.js?
2. Does this authenticated payroll application need SSR?
3. Does it have public SEO pages?
4. Is React Router enough?
5. Is TanStack Query worth it for server state?
6. Is another global state library needed?
7. Generated TypeScript API types or hand-maintained initially?

Strong starting position:

> Vite SPA for the authenticated payroll application unless a real SSR use case appears.

---

# 26. API schema/type sharing

Options:

### A — manual DTOs

Simple but can drift.

### B — OpenAPI in `salt-server`, generate TypeScript client/types

Better machine-checked contract, but tooling may spread.

### C — Rust-to-TypeScript generation only

Smaller but does not capture route/error semantics.

Questions:

1. Does the first slice need code generation?
2. Which crate owns API DTO definitions?
3. Are OpenAPI derives confined to `salt-server`?
4. Does CI verify generated artifacts?

Strong candidate:

> If OpenAPI is used, no OpenAPI/transport attributes enter `payroll`.

---

# 27. First web tracer bullet

Setup assumptions:

- one Employer exists;
- one Owner Operator exists;
- at least one Employment can be made payable.

Journey:

```text
1. Operator opens Salt.
2. Operator signs in.
3. Server establishes session.
4. Operator enters an authorized Employer.
5. Operator opens Payroll.
6. Operator creates next Ordinary run.
7. Salt displays all proposed Employments.
8. Operator optionally adds TaxableAllowance lines.
9. Operator clicks Calculate.
10. Salt displays per Employment:
       Basic Pay
       Taxable Allowances
       Gross
       Taxable Remuneration
       PAYE
       Employee SSC
       Employer SSC
       Net Pay
       warnings/refusals
11. Operator clicks Finalize.
12. Salt finalizes through `payroll-app`.
13. Operator sees immutable finalized result.
14. Refresh/re-login shows same finalized history.
```

Reversal/correction UI is not required for the first slice.

---

# 28. How much standing-data UI belongs here?

Options:

### A
Seed/bootstrap Employer + Employment; UI proves payroll only.

### B
Build complete Employer/Employment/Compensation/declaration setup first.

### C
Build only minimum screens required to create one payable Employment.

Strong candidate:

> Include minimum standing-data screens needed to create one payable Employment, then ordinary payroll. OpeningBalance and correction UX follow.

**GRILL THIS.**

---

# 29. Candidate UI navigation

Task-oriented:

```text
/login

/app/employers/:employerId/people
/app/employers/:employerId/people/:employmentId

/app/employers/:employerId/payroll
/app/employers/:employerId/payroll/:runId
```

Avoid table-oriented pages named after persistence concepts.

---

# 30. Browser state

Candidate split:

### Server state
Employments, PayrollRuns, calculations, finalized payroll, etc.

### UI state
Forms, modals, tabs, temporary acknowledgement selection.

### Authentication
Derived from a server session endpoint if opaque sessions are used.

### Active Employer
Strong candidate: EmployerId in URL is navigation source of truth.

Authorization remains server-side.

---

# 31. Failed calculation after refresh

Important UI-exposed question:

```text
Run is Draft.
Calculate refused because one Employment has PriorEmploymentUnknown.
Browser refreshes.
```

What does the run page show?

Options:

### A — refusal is not persisted
Refresh loses it.

### B — persist latest calculation attempt/refusal
Better UX but changes working-state model.

### C — compute readiness/refusals dynamically
Avoids persistence but risks duplicating calculator/app logic.

Do not let React store this as permanent local state.

**GRILL THIS HARD.**

---

# 32. Finalization UX

Finalization is the approval.

Candidate:

```text
Calculated run
  ↓
totals/results
  ↓
Finalize payroll
  ↓
simple confirmation:
"This creates immutable payroll history for N employees."
  ↓
POST finalize
```

Likely no re-enter password, no type-FINALIZE ceremony, no second-person approval.

If facts changed, finalization refuses with structured mismatch details and requires recalculation.

---

# 33. HTTP integration tests

Candidate:

```text
real PostgreSQL
+
real Axum Router
+
HTTP requests through router/test service
```

Prove:

1. unauthenticated request -> 401;
2. authenticated non-member cannot access Employer;
3. Owner can access own Employer;
4. Employer A cannot fetch Employer B's run by known id;
5. domain refusal returns stable code/details;
6. calculate → finalize works through HTTP;
7. finalized result reads back;
8. ActionLog actor comes from session identity, not request body.

Then one browser E2E test proves React.

---

# 34. Proxy/security assumptions

If Caddy fronts Salt:

```text
Internet
  ↓ TLS
Caddy
  ↓
salt-server
```

Settle:

- trust of forwarded IP headers;
- secure cookies;
- HSTS at proxy;
- CSP;
- frame protection;
- MIME sniff protection;
- JSON body limits.

Do not build a custom WAF.

---

# 35. Rate limiting

Main concern is login.

Questions:

- per-IP throttling?
- per-account throttling?
- Caddy or Axum?
- what is enough for first internet-facing deployment?

Do not build distributed rate-limit infrastructure, but do not expose unlimited password guessing.

---

# 36. Database roles

The runtime server should use the restricted application role from the persistence design.

Strong candidate:

```text
migration/admin DB credential
    !=
runtime application DB credential
```

Questions:

1. Does `salt-server` ever need owner privileges? Strong candidate: no.
2. Can session/identity tables be fully operated by restricted role?
3. Are grants explicit in migrations?
4. Are migration credentials absent from the runtime process?

---

# 37. Identity migrations

Likely:

```text
operator
employer_membership
session
```

Do not automatically mutate immutable historical actor columns into FKs.

Potential options include keeping historical actor text frozen while authorization uses OperatorId going forward.

**GRILL CAREFULLY.**

---

# 38. Bootstrap and first Owner

Options:

- explicit CLI/bootstrap command;
- first-start env bootstrap;
- direct SQL;
- public signup.

Strong candidate:

> Explicit bootstrap command creates first Operator, Employer and Owner membership. No public signup yet.

Questions:

- hosted SaaS or known-customer deployment?
- who creates later Operators?
- is invitations UI deferred?

---

# 39. Multi-Employer UX

Model membership correctly now.

But the first UI does not necessarily need a polished Employer switcher.

Question:

> Expose switching now, or only support one visible Employer while keeping data model multi-Employer-safe?

---

# 40. React deployment

Candidate:

```text
Caddy
  ├── /api/*  → salt-server
  └── /*       → React dist/
```

Alternatives:

- Axum serves static bundle;
- separate frontend host.

Strong candidate: Caddy serves the SPA and reverse-proxies `/api`.

**GRILL THIS.**

---

# 41. First product milestone

Candidate milestone:

> One Owner can bootstrap/sign in, create enough standing data for one Employment, create an Ordinary PayrollRun, calculate it, inspect the result, finalize it, sign out, sign back in, and inspect the same immutable finalized payroll.

Security condition:

> A valid Operator without Employer membership cannot discover or mutate that payroll even with valid resource ids.

Architecture condition:

> No Axum handler writes payroll tables directly or reproduces payroll rules.

---

# 42. Questions the grill MUST answer

## Identity

1. Operator/User/Account terminology?
2. Global or Employer-scoped identity?
3. Multiple Employers per identity?
4. Disable semantics?
5. Actor identity in ActionLog?
6. Historical actor FKs or frozen text?

## Authorization

7. v1 roles?
8. exact role capabilities?
9. authorization check on every Employer request?
10. how made difficult to forget?
11. 403 vs scoped 404?
12. RLS or not?

## Authentication

13. opaque session or JWT?
14. why?
15. session/token persistence?
16. expiry?
17. revocation/logout?
18. membership change effect?
19. first Owner bootstrap?
20. password reset now/later?

## Browser security

21. same-origin?
22. CSRF?
23. CORS?
24. production cookies?
25. login throttling?
26. body limits/security headers?

## Server

27. `salt-server` crate?
28. binary only or lib+bin?
29. pool ownership?
30. migrations at deploy/startup?
31. runtime DB role?
32. config?
33. health/readiness?
34. tracing?

## API

35. versioning?
36. first routes?
37. command routes?
38. error format?
39. machine error codes?
40. warning/acknowledgement contract?
41. idempotency/retry behaviour?
42. tenant-safe URL/query rules?

## Read models

43. exact first reads?
44. where do read models live?
45. run detail shape?
46. failed calculation after refresh?
47. finalized snapshot view?

## React

48. Vite or Next.js?
49. router?
50. server-state library?
51. generated API types?
52. minimum setup screens?
53. first payroll screens?
54. browser E2E?

## Scope

55. OpeningBalance UI now?
56. Reversal/correction UI deferred?
57. ActionLog UI deferred?
58. Multi-Employer switching visible now?
59. employee self-service excluded?
60. exact ready-for-spec condition?

---

# 43. Strong defaults for the grill to attack

1. Add `crates/salt-server`.
2. `salt-server -> payroll-app -> payroll`.
3. `salt-server` owns Axum, pool, config, tracing, auth, HTTP DTOs.
4. `payroll-app` gains purpose-built reads needed by UI.
5. Persist global Operator identities.
6. Persist EmployerMembership.
7. At most `Owner` and `PayrollOperator` initially.
8. No platform super-admin in normal API.
9. Opaque server-side sessions.
10. Secure + HttpOnly cookies in production.
11. No bearer credential in browser localStorage.
12. Same-origin production frontend/API.
13. Explicit CSRF defense.
14. Every Employer handler obtains an authorized Employer context.
15. Resource ids never authorize.
16. No RLS initially unless threat-model review justifies it.
17. Runtime server uses restricted DB role.
18. Migration credential separate from runtime.
19. Command endpoints for calculate/finalize rather than state assignment.
20. Stable error codes + structured details.
21. No SQL/internal errors to client.
22. Exact-period acknowledgement protocol; no `force=true`.
23. No raw frozen JSON as normal API response.
24. React + TypeScript + Vite unless SSR has a real need.
25. React Router.
26. TanStack Query unless a simpler approach is clearly enough.
27. Minimal global client state.
28. First UI includes minimum standing-data setup + Ordinary payroll flow.
29. Reversal/correction UI follows later.
30. HTTP integration test proves auth + tenant isolation + calculate/finalize.
31. Browser E2E proves login → ordinary payroll → finalized history.
32. No public signup initially.
33. Explicit first-Owner bootstrap.
34. Caddy serves SPA and proxies `/api` unless deployment requirements say otherwise.

---

# 44. Scenarios the grill should actively break

## A. Cross-Employer IDOR

Operator belongs to Employer A and learns Employer B's PayrollRunId.

Why can they not read or mutate it?

## B. Forged actor

Alice submits `"actor": "Bob"`.

Why does ActionLog still name Alice?

## C. Lost finalization response

Finalize commits, network response is lost, browser retries.

What does caller see? Why no duplicate payroll?

## D. Membership revoked during session

Operator logs in, membership is removed, old tab calls Calculate.

When does access stop?

## E. Divergence acknowledgement race

UI learns `[March, April]`; another change adds May; UI resubmits `[March, April]`.

Why is stale acknowledgement refused?

## F. Client invents state

Browser submits `"status": "Finalized"`.

Why is there no route that accepts this?

## G. Runtime DB misuse

Server attempts `UPDATE finalized_payroll`.

Why does PostgreSQL refuse?

## H. PAYE duplicated in TypeScript

A developer adds estimated PAYE logic for UX.

Why should this be rejected?

## I. XSS credential surface

If browser JavaScript is compromised, what auth credential can it steal?

## J. Failed calculation after refresh

Run is Draft because PriorEmployment is Unknown; browser refreshes.

How does user know what blocks payroll?

This must have a precise answer.

---

# 45. Tests the eventual spec will probably owe

## Authentication

- valid login creates session;
- wrong credentials do not;
- logout/revocation kills session;
- disabled Operator cannot continue;
- cookie/session properties asserted where practical.

## Authorization

- member can access own Employer;
- non-member cannot;
- known EmploymentId/RunId/FinalizedPayrollId from another Employer cannot cross boundary;
- role restrictions hold.

## Actor integrity

- ActionLog actor derives from authenticated identity;
- body cannot override it.

## API semantics

- malformed request != payroll refusal;
- refusal has stable code/details;
- DB failure maps to internal error without leakage;
- finalization mismatch is structured;
- divergence acknowledgement returns exact periods.

## HTTP tracer bullet

- authenticated Owner creates Ordinary run;
- calculates;
- reads working result;
- finalizes;
- reads finalized result;
- another Employer cannot access it.

## Browser

- login;
- minimal standing data;
- create run;
- calculate;
- inspect totals;
- finalize;
- reload and see finalized history.

---

# 46. ADRs likely to emerge

Potentially:

- authentication mechanism;
- tenant authorization boundary;
- HTTP/application separation;
- API error contract;
- frontend/deployment architecture if durable.

Do not create an ADR for every library.

---

# 47. Success condition

The design is settled when these stories have exact answers.

## Ordinary payroll

Alice signs in, is authorized for Employer E, creates March payroll, calculates, finalizes and refreshes.

Which HTTP requests happen? Which layer owns each decision? Where does actor identity come from?

## Tenant isolation

Alice belongs to Employer A and learns valid ids from Employer B.

Name the exact seam that prevents access.

## Data changes before finalization

Alice calculates. Another operator corrects CompensationTerms. Alice clicks Finalize.

What exact refusal crosses HTTP? What does React show? How does she recover?

## Divergence acknowledgement

Historical CompensationTerms correction affects three Live periods.

What exactly is returned, acknowledged and rechecked?

## Hostile client input

Browser supplies another EmployerId, forged actor, fake Finalized status and hand-computed PAYE.

Which are ignored/refused, and why can none become payroll truth?

## Authorization revocation

Bob loses membership while his session remains open.

Which next request fails and why?

If these are precise, proceed to `/to-spec` for **SPEC 1** (§0.3). Not one spec.

---

# 48. Desired `/grill-with-docs` output

The grill should leave this document with:

- settled identity model;
- settled Employer membership/role model;
- authentication mechanism;
- session/token semantics;
- first-Owner bootstrap path;
- CSRF/CORS/same-origin decision;
- exact tenant authorization seam;
- `salt-server` crate/process boundary;
- runtime vs migration DB role decision;
- minimum read/query surface in `payroll-app`;
- API route style;
- error/warning/acknowledgement contract;
- retry/idempotency behaviour for dangerous commands;
- decision on failed-calculation visibility after refresh;
- React architecture;
- first UI scope;
- deployment topology;
- HTTP integration-test strategy;
- browser E2E tracer bullet;
- durable rationale in ADRs where warranted;
- unresolved questions stamped instead of guessed.

Do not implement during the grill.

Do not add Axum routes during the grill.

Do not scaffold React during the grill.

Do not add JWT merely because it is common.

Do not introduce authorization concepts with no user story.

Do not weaken any `payroll-app` invariant to make HTTP easier.

~~The next artifact after the grill should be one implementation spec.~~
**Superseded by §0.3.** The grill found the single spec too large to review, and
found a boundary — identity is provable with no payroll routes at all — that
splits it honestly. The next artifacts are the three specs of §0.3, in order,
beginning with SPEC 1.
