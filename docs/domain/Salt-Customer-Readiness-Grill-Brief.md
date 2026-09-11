# Salt: customer readiness and UX — grill brief

Date: 7 September 2026. Owner: Alexander Dreyer / United Apps.
Revision: **updated after a `/grill-with-docs` session on 7 September 2026.** Thirty-eight decisions were put to the owner across ten rounds and settled. Section 2 now carries them. What was previously a recommendation is either an accepted decision or an explicitly remaining open matter — nothing is left as a silent assumption.
Repository state this revision was written against: `main` at **`fb33007d7d4c6de13b73f3ac87591f946c2ae45c`** ("Make the browser journey start its servers on CI"). The repository was inspected directly; §2.2 records what was found. No application code was changed and no tests were run during the grill.
Status: settled design record for the next milestone. It is not yet an implementation specification. `/to-spec` is deliberately **not** run yet.

## 1. Task for the receiving agent

The decisions in §2 are settled. **Do not reopen them.** Do not re-derive them, re-litigate them, or quietly widen them. If new evidence contradicts one, say so plainly and name the decision — do not silently change it.

What remains is: (a) the genuinely open legal and evidence questions in §9, and (b) turning §2 into bounded implementation specifications when the owner asks for `/to-spec`.

Read `AGENTS.md`, `CONTEXT.md`, the ADRs, `docs/domain/payroll-calculation.md`, `docs/domain/payroll-run-persistence.md`, `docs/domain/statutory-conformance.md`, `docs/domain/operator-auth-http-web-grill.md`, and issues #59 and #69. Current conformance amendments override superseded calculation text. Verify the repository at the current HEAD; `main` may have advanced past `fb33007`.

The supplied `Project-coding-chat-history.txt` records an older .NET/Blazor implementation. `Pay-roll-research-and-DB-tables.txt` preserves original product intentions but contains assistant-generated legal claims, pricing and architecture proposals that are not authoritative. Do not reinstate its generic formula engine or table list over the Rust design.

An owner choosing a behaviour does not establish that it is legally correct. Every decision below that touches statutory arithmetic is stamped as a `SaltPolicy` where no published rule was found, exactly as `docs/domain/statutory-conformance.md` requires.

## 2. Settled decisions

### 2.1 The decision table

All thirty-eight were confirmed by the owner on 7 September 2026.

| # | Decision | Settled choice |
|---|---|---|
| D1 | Next milestone | **Internal prototype.** Two consecutive months, end to end, on fictional data, with real screens. Statutory reports remain unbuilt. The customer-demo floor (earnings, deductions, NamRA reporting, reconciliation) is **not** relaxed — it is deferred to the milestone after this one. |
| D2 | Evidence source | The supplied Sage VIP payslip plus the owner's salaried network-technician experience. A practitioner contact is busy; validation questions are collected for later and are **never** a blocker. |
| D3 | Pay frequency | **Monthly only.** Weekly and fortnightly are refused with a clear message, not silently unsupported. |
| D4 | Design system | **shadcn/ui with Tailwind.** Mantine rejected. The owner's own Impeccable installation stays available for critique. |
| D5 | Issue #69 | Fixed on its own, before new work. A duplicate `CompensationTerms` effective date must answer an actionable conflict, not a 500. |
| D6 | Overtime | **In scope for this milestone.** |
| D7 | Non-statutory deduction | **Supported.** Withheld after tax. It never touches PAYE or social security arithmetic. Medical aid is the driving example. |
| D8 | Payslip delivery | Rendered **on demand**, downloadable or printable, **never stored**. If it is needed again it is re-rendered. |
| D9 | Visual exploration | **One direction only.** `DESIGN.md` pins the tokens; the look lives in CSS variables, so changing it later is a token swap, not a redesign. |
| D10 | Hourly-paid employees | **Out of this milestone.** The rate concept is designed so they slot in later without rewriting `CompensationTerms`. |
| D11 | Corrections | **Exposed in the browser.** Both the HTTP routes and the screens are new work. |
| D12 | Payslip template version | Frozen on each `FinalizedPayroll`, beside `SaltVersion`. |
| D13 | Navigation | **People / Payroll / Employer.** No Reports tab until reports exist. |
| D14 | Overtime input | **Hours per multiplier.** Salt computes the money. The T&A system owns the rules that decide which hours earn which multiplier. |
| D15 | Allowance naming | A free-text **label** plus a separate **classification**. The label never affects tax or contributions. |
| D16 | PAYE refund | The existing refusal (`statutory-conformance.md` §5.9) stands and is shown plainly on screen. `ADR-0001` is **not** reopened. |
| D17 | Adoption | The prototype starts **mid tax year**, exercising `OpeningBalance` and `SaltCoverageStart`. |
| D18 | Salary to hourly rate | `basic pay × 12 ÷ 52 ÷ OrdinaryHours`, with `OrdinaryHours` recorded per Employment on `CompensationTerms`. **A `SaltPolicy`, stamped `NEEDS CONFIRMATION`.** |
| D19 | Overtime category | **The multiplier is the classification.** A free-text label carries the human reason ("Sunday overtime") and never affects money. |
| D20 | Particulars | Employer address and registration numbers, and Person identity number and address, are stored as real fields with real forms. Not faked in the renderer. |
| D21 | Fixture | **Five** fictional employees, covering every path chosen here. |
| D22 | Document stability | Employer and Person particulars **freeze into the `FinalizedPayroll`**. A later address or name correction can never rewrite an old payslip. This is `ADR-0004` applied, not a new principle. |
| D23 | Leave | **Out of this milestone entirely.** No leave block on the payslip. |
| D24 | Attendance import | **Out.** Overtime hours are typed on the payroll worksheet. |
| D25 | Roles | Employer particulars are **Owner-only**. `require_role` stops being dead code. |
| D26 | Master-data correction | Person names and particulars become **correctable with a stated reason**, the `CompensationTerms` pattern. Safe because of D22. |
| D27 | Browser tests | **Several focused tests**, not one growing journey. |
| D28 | ADRs | **Two.** See §2.4. |
| D29 | Deliverable | This document and `CONTEXT.md`. **No GitHub issues, no `/to-spec` yet.** |
| D30 | PDF rendering | **Server-side, in Rust.** The browser downloads a real file. |
| D31 | Multipliers | **Fixed set: 1.5 and 2.0.** A third requires a code change and a deliberate decision, exactly as a third `Earning` kind does. |
| D32 | Employer-paid medical aid | **Declared and refused**, beside the four tax-deductible kinds. The employee's own medical aid deduction (D7) still works. |
| D33 | Leaver | **End date only.** Salt stops proposing them. Leave payout, notice pay and severance are refused. |
| D34 | Run outputs | Payslips, **payroll register** and **payment summary**, on the finalized run screen — not behind a Reports tab. |
| D35 | Standing items | Effective-dated on the Employment, proposed into every run. One-off items are typed on the run and never recur. |
| D36 | Employee entry | **Typed by hand.** No CSV import. |
| D37 | Payment summary | **PDF and screen only.** No CSV and no bank file — a CSV that resembles a bank import but is not one is a trap. |
| D38 | One-month override | A run may change or remove a proposed standing item **for that run only**, without touching the standing record. |

### 2.2 What the repository actually holds at `fb33007`

Established by direct inspection, not inference.

| Area | Present | Absent |
|---|---|---|
| Earnings | `BasicPay`, `TaxableAllowance` (`crates/payroll/src/earning.rs:13`) | Overtime, any labelled allowance |
| Deductions | `Statutory::PAYE`, `Statutory::SocialSecurity` (`crates/payroll/src/deduction.rs:13`) | Any non-statutory deduction — **including medical aid** |
| Employer contribution | Employer social security is calculated (`crates/payroll/src/calculation.rs:405`) | Any other contribution |
| Refused deduction kinds | Approved pension, provident, retirement annuity, education policy (`crates/payroll/src/unsupported_deduction.rs:15`) | Medical aid is **not** in the list |
| Employer record | id, pay schedule, name (`migrations/0001`, `0027`) | Address, registration numbers, rename path |
| Person record | full name, employer-scoped, **append-only** (`migrations/0031`) | Identity number, address, any correction path |
| CompensationTerms | `basic_pay`, effective-dated, correctable (`migrations/0003`) | Ordinary hours, hourly rate |
| Pay lines | Per run only (`migrations/0013_payroll_run_earning.sql`) | Any standing/recurring item |
| Corrections | Full engine support (`payroll-app/src/correction.rs`, `reversal.rs`) | **No HTTP route at all** (`crates/salt-server/src/payroll_runs.rs:116`) |
| Roles | `Owner` / `PayrollOperator`, `require_role` written and tested | Marked `dead_code` — no Owner-only route exists (`authorized_employer.rs:79`) |
| Web | Login, people, employment facts, payroll run, finalized payroll, workings | Navigation menu, payslip, register, payment summary, any report |
| Styling | 69 lines of hand-written CSS (`web/src/index.css`) | Component library, Tailwind, design tokens |

**Two findings deserve naming as risks, not just gaps.**

1. **Medical aid is a silent wrong answer today.** It is not a supported deduction and not a refused kind. An Employment with one has nowhere to record it, so Salt would report a net pay that is too high and say nothing. D7 closes this.
2. **A misspelled Person name is currently unfixable.** `person` is append-only with `UPDATE` revoked. D26 closes it, and D22 is what makes closing it safe.

### 2.3 The supported Employment profile for this milestone

An Employment is supported when **all** of the following hold. Anything else is refused explicitly, on screen, before the operator invests in setup.

Supported:

- Monthly pay frequency (D3).
- Salaried pay basis, with `BasicPay` from effective-dated `CompensationTerms`.
- `OrdinaryHours` per week recorded on `CompensationTerms`, used only to derive an hourly rate (D18).
- Labelled taxable allowances, standing or one-off (D15, D35).
- Overtime hours at multiplier 1.5 or 2.0 (D14, D19, D31).
- Non-statutory deductions, standing or one-off — medical aid being the driving case (D7, D35).
- PAYE and social security, as already shipped.
- Mid-year adoption through `OpeningBalance` and `SaltCoverageStart` (D17).
- A recorded end date, with their last month paid as an ordinary month (D33).

Refused, with a stated reason:

- Hourly pay basis (D10), weekly and fortnightly frequency (D3).
- Leave of any kind, paid or unpaid (D23).
- Approved pension, provident fund, retirement annuity, education policy — unchanged.
- Employer-paid medical aid as a taxable benefit (D32).
- `PriorEmployment` with known figures or unknown — unchanged, `SC-OPEN-4`.
- Any final-pay component: leave payout, notice pay, severance (D33).
- Any correction that would produce a PAYE refund (D16).

### 2.4 The two ADRs this session owes

Both meet all three of the domain-modelling criteria: hard to reverse, surprising without context, and the result of a real trade-off.

**ADR-0021 — A payslip is rendered on demand and never stored.** The server renders PDF bytes in Rust from the `FinalizedPayroll` (D8, D30). Nothing is written to a document store. The rejected alternative is retaining the original bytes forever, which most payroll products do. The decision is only safe because two things freeze with the payroll: the `PayslipTemplateVersion` (D12) and the Employer and Person particulars (D22). Record that dependency explicitly — an ADR that omits it makes an unsafe decision look safe. The consequence to state plainly: **every retired template renderer must be kept alive forever**, because an old payslip may be re-rendered at any time.

**ADR-0022 — Overtime is hours at a fixed multiplier, priced by a derived hourly rate.** Salt receives hours per multiplier, never money (D14). The multiplier set is closed at 1.5 and 2.0 (D31). For a salaried Employment the hourly rate is derived: `basic pay × 12 ÷ 52 ÷ OrdinaryHours` (D18). The rejected alternative was accepting money amounts, which needs no divisor but pushes the arithmetic back into a spreadsheet — the exact problem Salt exists to remove. The divisor is a `SaltPolicy` stamped `NEEDS CONFIRMATION`: no published Namibian rule prescribing it was found. `OrdinaryHours` is recorded per Employment precisely so the assumption is visible and dated rather than hidden in a constant such as 173.33.

Freezing the Employer and Person particulars into the `FinalizedPayroll` deliberately gets **no** ADR. It is `ADR-0004` applied to two more fields, and a reader who knows `ADR-0004` will find nothing surprising in it.

## 3. Customer proposition and evidence

Recommended proposition, unchanged: an employer can bring existing payroll into Salt, understand what changed this month, resolve problems, produce dependable payslips and filing information, and repeat the process without a developer.

This remains a plausible opportunity, **not validated demand**. Public sources establish competition and expected capabilities, not willingness to pay or dissatisfaction with existing products.

| Evidence checked | Product implication |
|---|---|
| Grans Namibia advertises Premium Pay cloud access, multiple payroll frequencies, employee self-service and local reporting [S1]. | Cloud access and modern-looking screens alone will not differentiate Salt. |
| Totus describes local Sage/VIP and D-Bit services, including deductions, contributions and reporting [S2]. | Salt competes with service and payroll expertise as well as software. Assisted migration and support matter. |
| Sage lists Namibia in its Pastel Payroll footprint [S3]. | Do not assume Namibia is an unserved market. |

D2 settles how the design proceeds without a pilot employer: work from the supplied VIP payslip and the owner's own salaried experience, and keep a written list of questions for the practitioner. Discovery is still owed before the milestone **after** this one — roughly 5–20 employees paid monthly, owner or administrator operating payroll, currently on spreadsheets. That size is a hypothesis, not an agreed limit. A small headcount does not imply simple statutory obligations.

No pricing is settled. No verified like-for-like Namibia price comparison was established.

## 4. Milestone and release boundaries

**This milestone (settled, D1): an internal prototype.** Five fictional employees, two consecutive monthly payrolls, mid-year adoption, standing and one-off pay items, typed overtime hours, one correction, one deliberately refused PAYE refund, payslips, payroll register and payment summary — all driven from a browser with no SQL, no curl and no developer intervention after provisioning.

The three outcomes stay distinct, and the later two are **not** relaxed:

1. **Internal prototype (now).** Fictional data demonstrates design and workflow. Statutory reporting remains unbuilt. This is a deliberate, stated exclusion — not a claim of readiness.
2. **Customer demo (next).** The owner's earlier requirement stands in full: earnings, deductions, NamRA reporting and reconciliation. It must not quietly become a screenshot demo.
3. **Live pilot (later).** Reconcile independently, resolve the relevant statutory uncertainties, prove the whole selected employer's payroll is supported, and complete hosted operational readiness. A successful two-month prototype establishes nothing about year-end readiness.

The narrow engine scope proved the architecture. It is not an adequate first *product* scope, and D6, D7, D11, D34 and D35 are the beginning of closing that. Never conceal an excluded employee, and never ask a user to disguise a bonus, overtime or reimbursement as an allowance.

## 5. Work packages, in proposed order

### A. Correctness and particulars

Fix #69 first, on its own (D5): a duplicate `CompensationTerms` effective date must answer an actionable conflict [R2].

Add Employer particulars — address and registration numbers — and Person particulars — identity number and address (D20). Editing Employer particulars is Owner-only (D25), which is the first route to use the already-written `require_role`. Make Person names and particulars correctable with a stated reason (D26).

Freeze Employer and Person particulars into the `FinalizedPayroll` snapshot at finalization (D22), beside the existing `PayrollInput`, `PayrollRules` and `SaltVersion`. Add the `PayslipTemplateVersion` to the same snapshot (D12).

Record actual employment start dates and the adoption period. Display saved compensation terms, declarations and opening balances after reload. Keep the distinction between a salary change and correcting a recorded fact. Never turn blank evidence into a confirmed zero. `OpeningBalance` (same employment, same employer) stays firmly distinct from `PriorEmployment` (another employer).

Employees are typed in by hand (D36). No CSV import, no import preview, no row-error machinery — there is no real customer file to build against yet.

### B. Connected working interface

Establish persistent **People / Payroll / Employer** navigation and an obvious active employer (D13). No Reports tab: a door that does not open should not be drawn. Preserve membership-based employer switching and clear caches when switching; a single-employer operator should meet no unnecessary selection step.

Design employee detail and the monthly payroll worksheet together. The worksheet shows employees, readiness and blockers, basic pay, additions, deductions and net pay, with **server-provided** totals. Provide previous-period comparisons only where the server supplies meaningful values. Detail exposes the workings without making ordinary users read them first.

The server proposes valid next periods and dates. A blocker links to its remedy and returns to the originating run. Explain exclusions and record reasons. **No client-side estimated PAYE and no client-side net pay** — React owns no payroll arithmetic and consumes structured server error codes.

Leave a place in the worksheet layout for an hours column even though hourly pay is out of scope (D10), so the later change is a fill rather than a redesign.

### B2. Overtime and standing items

**Overtime (D6, D14, D19, D31).** The operator types hours per multiplier on the worksheet — for example twelve hours at 1.5 and four at 2.0. The multiplier is the classification; a free-text label such as "Sunday overtime" carries the reason and never affects money. The multiplier set is closed at 1.5 and 2.0.

The hourly rate is derived from the salary (D18) using `OrdinaryHours` on `CompensationTerms`. Both the derivation and its `SaltPolicy` stamp must appear in the calculation trace — an operator asking "why is this N$412.50" must be able to see the divisor and that Salt chose it.

**Overtime and the social security base is settled law, not a choice.** `docs/domain/statutory-conformance.md` §3.3 records that the Social Security General Regulations define `basic wage` as remuneration for ordinary work, **excluding** allowances, overtime, additional Sunday and public-holiday pay, and night-work pay. An `Overtime` earning therefore feeds gross and taxable remuneration and **never** the social security base. Extend the existing exhaustive two-way table in `RemunerationBases::accumulate` — the compiler will refuse the build until the new kind states its effect on all three bases, which is the intended behaviour.

**Standing items (D35, D38).** An Employment carries effective-dated standing Earnings and standing Deductions. Every run proposes them. A one-off item typed on a run never recurs. A run may change or remove a proposed standing item **for that run only**, leaving the standing record untouched — a one-month medical aid reduction must not rewrite a person's permanent record. This is the largest single piece of engine work in the milestone: a new table, new run-proposal logic, and an override that is visible in the calculation trace.

**Attendance imports and leave are both out (D23, D24).** They stay in *product* scope for later. Their exclusion here is a stated boundary, not a claim they are unnecessary. When leave does arrive, the hard part is not display: accrual must not fire twice when a run is recalculated, retried, reversed or replaced, and historical payslip leave figures must stay stable after later adjustments.

Design the worksheet, overtime entry and standing-item override together, before visual polish. They are the working product, not decoration added after the forms are complete.

### C. Outputs

Produce, from the finalized run screen (D34): individual and batch payslips, a payroll register (every employee in the run, with totals) and a payment summary (names and net pay).

The payslip is a **PDF rendered server-side in Rust** (D30), on demand, downloadable or printable, **never stored** (D8). It is re-rendered whenever needed. This is only safe because the `PayslipTemplateVersion` and the Employer and Person particulars are frozen with the payroll (D12, D22). Keep every retired template renderer alive.

The payment summary is PDF and screen only (D37). No CSV, and no bank file — a bank-specific format needs verification Salt does not have, and a CSV resembling one would be mistaken for one.

Monetary results come from finalized records. Mark draft previews clearly. Finalizing payroll does not mean a bank transfer occurred; producing a payment summary does not mean anyone was paid.

**Before fixing the payslip layout, map every required field to a source.** The Labour General Regulations, regulation 3 and Annexure 1, specify more than gross, deductions and net: employee identity, employer addresses, wage basis, period, and hours and remuneration categories also appear [S6]. D20 exists to supply the identity and address fields. ⚠️ **See `Q-OPEN-7` in §9:** whether Annexure 1 requires *named* hour categories rather than bare multipliers was not verified from primary source in this session, and D19 could need revisiting.

Statutory reporting — ETX, PAYE5, SSC, VET, ECF — is **out of this milestone** and is the defining content of the next one. Nothing here is a claim that it is close.

### D. Corrections and continuity

Expose the existing correction, reversal and replacement engine through deliberate operator workflows (D11). **There is currently no HTTP route for a correction at all**, so both the routes and the screens are new; the expensive half is already built and tested.

Show what is being corrected, the reason, the affected periods, the recalculation consequences, and the link between the original and its replacement. Inspect the existing application contracts before inventing routes or new domain machinery.

Distinguish correcting payroll history from recovering money already paid, and from amending a submitted statutory return. A database reversal makes neither of the latter two true.

**The PAYE refund wall is deliberate and stays (D16).** If a correction lowers recalculated year-to-date liability below PAYE already withheld, `calculate()` refuses. The UI must show that refusal plainly and stop. `ADR-0001` is not reopened. The prototype fixture includes a refund case **on purpose**, so the owner sees the wall and can judge whether it is acceptable before a customer meets it.

Prove the second month uses the correct history after a pay change and a correction. A recorded end date implements no final pay (D33).

### E. Hosted live readiness

Unchanged, and entirely out of this milestone. Before live operation: operator provisioning and account recovery, employer membership changes and revocation, migrations, reliable backups and a demonstrated restore, protected document access, useful audit trails and support references, and a documented update and incident process. Keep salary and identity details out of routine logs. Define retention and customer data export against verified obligations. These are payroll service requirements, not a reason to build an enterprise administration platform.

## 6. Calculation readiness

The conformance record's distinction between law, provisional Salt policy and algorithm tests is retained absolutely [R3]. It records unresolved per-period PAYE method, rounding, part-month SSC, prior-employer treatment and a minimum-base discrepancy. Its current method uses cumulative scaled thresholds; annual mathematical reconciliation is not proof that its monthly withholding method is accepted.

**This milestone adds one new `SaltPolicy`, and it must be stamped like the others: `SC-OPEN-6`, the salary-to-hourly divisor (D18).** No published Namibian rule prescribing a divisor was found. Recording `OrdinaryHours` per Employment makes the assumption visible and dated instead of hiding a constant. It is Salt's choice and must never be described as statutory.

**This milestone changes no existing statutory arithmetic.** Overtime feeds gross and taxable but not the social security base — that is §3.3 applied, not a new rule. Non-statutory deductions are withheld after tax and touch neither the PAYE nor the social security calculation. New test names must carry the existing prefixes honestly: `salt_policy_*` for the divisor and for any period outcome, never `statutory_*`.

Required evidence work, still owed: resolve the open items with authoritative guidance and a practitioner, cross-check anonymized cases independently, and investigate any disagreement before changing code. The repository records the September 2026 SSC ceiling transition and future effective dates; inspect its provenance and test a boundary scenario. This document does not claim that every shipped statutory table has been independently revalidated.

Exclusions that remain permanent-looking but are not permanent: non-monthly basic pay, bonuses, commission, benefits, prior-employer income and PAYE refunds. They are capability gaps for the milestones after this one. Never replace explicit classification with a taxable checkbox or an arbitrary formula field — D19 and D31 exist to hold that line for overtime, and D15 holds it for allowances by keeping the label strictly separate from the classification.

## 7. UX and styling

**Settled: shadcn/ui with Tailwind (D4).** Issue #59 excluded a design-system choice [R1]; that was a scope boundary for the tracer bullet, not a permanent ban. `web/package.json` at `fb33007` has no component library and no styling framework, and `web/src/index.css` is 69 lines of hand-written CSS [R4].

The owner's own Impeccable installation stays available for critique. Mantine was considered and rejected. No paid design subscription and no Figma handoff is necessary. Tools supply components and design guidance; they do not decide payroll workflow and they do not prove accessibility.

Visual direction: a calm, professional, light interface with restrained accent colour, strong typography, clear labels and compact readable tables. Right-align money, use tabular numerals, show N$ explicitly where needed, and use unambiguous human-readable dates. Put the current task and next action ahead of decorative dashboard cards. Preserve visible keyboard focus, meaningful labels, readable contrast, and errors that do not rely on colour alone.

Design sequence (D9 replaces the previous two-direction exploration):

1. Write a small repository `DESIGN.md`: audience, task flow, tokens, typography, spacing, density, form/table/dialog conventions and copy rules.
2. Install shadcn/ui and Tailwind, and build **one** direction. The look lives in CSS variables, so revising it later is a token change rather than a redesign — which is the reason shadcn was chosen over Mantine.
3. Prove the journey with loading, empty, invalid, blocked, stale, partially calculated, finalized and failed-request states. Inspect at laptop and narrow widths; prioritize efficient desktop payroll operation without making mobile unreadable.
4. Implement the components as part of complete vertical slices; use Impeccable to critique clarity and consistency, then polish each working slice.
5. Observe a target operator completing the task. Record confusion, lost work, wrong actions and help required. Visual attractiveness is necessary but insufficient.

The first version defers dark mode, animation-heavy effects and a theme editor. Network failures must preserve deliberate data-entry work where safe and give a truthful save and retry state. Hosted web does not imply offline payroll processing.

## 8. Acceptance evidence

**Five fictional employees, in one employer (D21):**

1. Plain salary only.
2. Salary plus a standing labelled taxable allowance ("standby allowance", after the supplied payslip).
3. Salary, overtime hours at both 1.5 and 2.0, and a standing medical aid deduction.
4. A mid-year joiner with an actual start date.
5. A leaver with an end date, whose last month is an ordinary payroll.

Use fictional equivalents throughout. Do not copy personal identifiers or real salary amounts from the supplied payslip into repository fixtures.

**The exercise must cover:** mid-year adoption with `OpeningBalance` and `SaltCoverageStart` (D17); reload of every saved fact; two consecutive periods; a salary change at a valid boundary; a standing item overridden for one month only (D38); a one-off earning proven not to recur (D35); a justified correction through the browser (D11); a deliberately refused PAYE refund (D16); a refused unsupported case so eligibility is demonstrated honestly; and individual plus batch payslips, the payroll register and the payment summary (D34).

**Verify:** finalized money matches payslips, register and payment summary; no double-counting of reversals or opening balances; overtime never reaches the social security base; a re-rendered payslip is byte-stable in content after an Employer address change (D22); and a correction leaves the second month's history correct.

Inspect layout with 20 and 50 rows, without implying those employers' pay profiles are supported.

**Browser tests are several focused specs, not one growing journey (D27):** mid-year adoption; an ordinary run; a correction; a refused refund; payslip and outputs. One giant test reports that something broke; focused tests name what. Risk-focused tests should extend the existing unit, integration and browser seams. Do not proliferate tests that merely mirror presentation code. Require employer-isolation checks for every newly added output route, and meaningful stale-calculation and retry coverage.

**Evidence of success for this milestone:** a target operator completes both months from a browser with no developer intervention, every figure reconciles across the three outputs, and every refused profile is refused visibly and with a reason.

## 9. Genuinely remaining open matters

Nothing here blocks the prototype. Everything here blocks the customer demo or the live pilot.

**Statutory and evidence — needs primary sources.**

| # | Question | Status |
|---|---|---|
| SC-OPEN-1 | How is one period's PAYE derived from the annual table? | `NEEDS NAMRA CONFIRMATION`, policy §5.1 |
| SC-OPEN-2 | Prescribed rounding for PAYE or SSC? | `NEEDS NAMRA CONFIRMATION`, policy §5.8 |
| SC-OPEN-3 | Do the SSC floor and ceiling apply in full to a part-month joiner or leaver? | `NEEDS SSC CONFIRMATION`, policy §5.7 |
| SC-OPEN-4 | Treatment of prior-employer remuneration and PAYE; is a directive or certificate required? | Refused, §5.6 |
| SC-OPEN-5 | SSC minimum earnings base — N$300 or N$500? | Implemented as N$500, flagged |
| **SC-OPEN-6** | **What divisor converts a monthly salary to an hourly rate for overtime?** | **New. `SaltPolicy` per D18, `NEEDS CONFIRMATION`** |
| **SC-OPEN-7** | **Is Salt correct that an employee's own medical aid premium gets no relief against taxable income?** | **New. `SaltPolicy` per issue #78, `NEEDS NAMRA CONFIRMATION`** |
| **Q-OPEN-7** | **Does Labour General Regulations Annexure 1 require *named* hour categories (overtime, Sunday, public holiday) on a payslip, rather than bare multipliers?** [S6] | **New. Not verified from primary source. Could require revisiting D19.** |
| **Q-OPEN-8** | **Are 1.5 and 2.0 the correct and only statutory overtime factors in Namibia?** | **New. Assumed from the owner's practice, not verified. Could require revisiting D31.** |
| Q-OPEN-9 | How is an employer-paid medical aid benefit valued for tax? | Refused under D32, so not blocking |
| Q-OPEN-10 | Is the calculator's work period the reporting month? Pay date versus deduction date, non-calendar periods, delayed pay, correction periods, year-end boundaries. | Deferred by the existing conformance design; blocks reporting |
| Q-OPEN-11 | VET applicability. NTA states the levy applies at annual payroll of N$1 million or more, subject to exemptions [S7]. Ten people on N$10,000 monthly reach N$1.2 million before other remuneration. | Blocks reporting. VET and ECF are employer costs, never employee deductions |
| Q-OPEN-12 | ECF classifications, rates and outputs. | Not established by any review to date |
| Q-OPEN-13 | Current ETX and PAYE5 operational templates; the difference between NamRA's employee-tax page and the ITAS FAQ on annual reconciliation obligations [S4, S5]. | Obtain current templates before claiming compatibility. Keep internal reconciliation as a firm product need; verify external annual filing rather than declaring one |
| Q-OPEN-22 | What does Namibian law require when a voluntary deduction (e.g. a medical aid premium) exceeds the net pay available to withhold it from — a priority order, a cap, a protected-earnings floor, carry-forward? | New (issue #78). Refused rather than guessed at: none of these is invented |

**Product and commercial — needs the owner or a customer.**

| # | Question |
|---|---|
| Q-OPEN-14 | Which willing employer eventually validates the complete workflow? Which independent Namibian payroll practitioner reviews the open statutory items? |
| Q-OPEN-15 | Which earnings and deductions occur in that employer's ordinary year, including December and leavers? Classify each as required now, later, or unsupported. |
| Q-OPEN-16 | Can anonymized Binary City and Jarrison payroll export samples be obtained, with product version and export configuration? [S8, S9] Named vendor compatibility requires a real fixture per claimed format. |
| Q-OPEN-17 | Hourly pay basis, and weekly or fortnightly frequency. Accepting overtime settled neither. |
| Q-OPEN-18 | Leave: types, working-day or hour units, policy cycles, carryover, caps, rounding, unpaid-leave pay effects, leaver treatment, and when accrual posts relative to recalculation and correction. |
| Q-OPEN-19 | Who checks and finalizes payroll, who pays it, and are separate approval permissions needed for a pilot? Which payment output is actually needed? |
| Q-OPEN-20 | What multi-company setup is required beyond existing authorized switching? Preserve current isolation; defer a bureau-wide dashboard unless demanded. |
| Q-OPEN-21 | What support and commercial arrangement makes a pilot viable? No price, billing, availability promise or launch date is settled. |

Technical design questions are resolved by repository inspection and focused proposals, not by asking the owner: new read and write contracts; the standing-item table and its run-proposal semantics; the frozen-particulars snapshot shape; the reporting period model; reversal and report amendment semantics; output versioning; and production operational ownership. Document choices and rejected alternatives without rebuilding already-settled architecture.

## 10. Explicitly deferred

Deferred to a later milestone, and each is a boundary rather than a prohibition: hourly pay basis; weekly and fortnightly payroll; leave in every form; attendance file import; raw clock, shift and break processing; direct attendance APIs and device integrations; statutory reporting outputs; CSV migration of employees and opening balances; bank-specific payment files.

Deferred indefinitely: Desktop and Tauri, offline sync; a general rules or formula designer; multi-country support; broader HR and employee leave-request portals; employee self-service; automatic emailing or WhatsApp delivery; bank execution; direct NamRA submission; an in-product LLM assistant; ERP platform expansion; self-service subscriptions.

Required payroll effects of leave or benefits cannot be ignored simply because full management modules are deferred — which is exactly why D23 refuses leave outright rather than half-modelling it, and why D32 refuses the employer-paid medical aid benefit rather than guessing its value.

## 11. Historical payslip evidence

The owner supplied `011-7003-20191231-0.pdf`, a Sage VIP payslip dated 31 December 2019 for salaried network-technician work. It shows salary and a standby allowance; PAYE, social security and medical-aid deductions; a separate company-contributions total; year-to-date taxable earnings and tax; and annual-leave cycle end, opening balance, taken, accrued, closing balance and entitlement. Overtime is a user requirement but does not appear on this particular payslip.

Annual entitlement is shown as 20 and period accrual as 1.6667, consistent with 20/12. The opening balance cannot be explained from this single payslip and must not be inferred from the engagement date alone. The company-contributions total is not itemized, so its component calculations cannot be reconstructed.

**What this document drove in the decision set:** the standby allowance drove D15 (label plus classification); the medical-aid deduction drove D7 and exposed the silent-gap risk in §2.2; the leave block drove D23 (refuse rather than half-model); the unitemized company-contributions total drove D32 (refuse rather than guess a valuation).

Use the example to identify required concepts and output fields, never to certify current law or a calculation algorithm. Do not copy personal identifiers or actual salary amounts into repository fixtures.

## 12. Sources and limitations

Sources checked on 7 September 2026. Vendor claims establish advertised functionality, not independently audited product quality. Public website review cannot validate willingness to pay. The legal sources identify design requirements and follow-up work; **this is not a statutory sign-off.**

Repository facts in §2.2 were established by direct inspection of `main` at `fb33007d7d4c6de13b73f3ac87591f946c2ae45c`. `main` may advance; verify before implementing.

- [R1 — Salt issue #59](https://github.com/xander365/salt/issues/59): browser tracer-bullet scope. Open at the time of this revision.
- [R2 — Salt issue #69](https://github.com/xander365/salt/issues/69): duplicate compensation-term error. Open at the time of this revision.
- [R3 — Statutory conformance record](https://github.com/xander365/salt/blob/main/docs/domain/statutory-conformance.md).
- [R4 — Frontend dependencies](https://github.com/xander365/salt/blob/main/web/package.json).
- [R5 — Calculation domain design](https://github.com/xander365/salt/blob/main/docs/domain/payroll-calculation.md).
- [R6 — Payroll run persistence design](https://github.com/xander365/salt/blob/main/docs/domain/payroll-run-persistence.md).
- [S1 — Grans Namibia Premium Pay](https://www.gransnamibia.com/premium-pay-payroll/).
- [S2 — Totus payroll services](https://totusconsult.com/cpt_services/industrial-relations/).
- [S3 — Sage Pastel Payroll](https://www.sage.com/en-za/products/sage-pastel-payroll/).
- [S4 — NamRA ITAS FAQ](https://www.itas.namra.org.na/faq).
- [S5 — NamRA employee tax](https://www.namra.org.na/tax-types/page/employee-tax-30120-30120/).
- [S6 — Government Gazette 4151, Labour General Regulations, regulation 3 and Annexure 1, pages 3 and 9–10](https://nsi.com.na/wp-content/uploads/2026/03/Regulations-Labour-Act-11-of-2007.pdf).
- [S7 — NTA VET levy](https://www.nta.com.na/vet-levy/).
- [S8 — Binary City payroll integration](https://www.bcity.me/bctime-beyond-basics): advertises file download and upload integration; exact Salt-compatible format unverified.
- [S9 — Jarrison payroll exports](https://www.jarrison.systems/software-integration/): advertises configurable text and ASCII payroll exports; exact customer export layouts unverified.
- [D1 — Official shadcn/ui skill](https://ui.shadcn.com/docs/skills).
- [D2 — shadcn/create](https://ui.shadcn.com/create).
- [D3 — Impeccable](https://impeccable.style/).
- [D4 — Mantine LLM documentation](https://mantine.dev/guides/llms/): considered and rejected under D4.

**Limitations of this revision specifically.** No web source was fetched during the grill; §3 and §12 carry forward the earlier research unchanged. Annexure 1 was not re-read from primary source, which is why `Q-OPEN-7` exists. The 1.5 and 2.0 overtime factors come from the owner's practice, not from a verified statute, which is why `Q-OPEN-8` exists. No code was changed and no tests were run.
