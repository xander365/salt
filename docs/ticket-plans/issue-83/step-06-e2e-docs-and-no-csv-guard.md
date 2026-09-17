# Step 6 — Browser test, docs, and the no-CSV guard

Read `README.md` first. Step 5 must be done.

## Read these files

- `AGENTS.md` — "Browser journey suite" (how to build and run e2e).
- `e2e/tests/payroll-journey.spec.ts` — setup flow and the payslip download
  assertion added in #82.
- `e2e/playwright.config.ts`, `e2e/global-setup.ts`.
- `CONTEXT.md` — `PayrollRegister`, `PaymentSummary`, `Payslip` entries.
- `docs/domain/payroll-run-persistence.md` — search for "register",
  "payment summary", "payslip".
- `docs/adr/0021-a-payslip-is-rendered-on-demand-and-never-stored.md`.

## Build

### 1. `e2e/tests/outputs.spec.ts` (spec §Seam 4, item 5)

Accessible role and name only — never CSS classes or test ids.

- Set up one Employer with two employments and finalize one month through the
  UI. Reuse the journey spec's steps; if they are inline, extract shared
  helpers into `e2e/tests/helpers/` and use them from both specs (do not
  change the journey spec's behaviour).
- From the finalized run screen:
  - download all payslips → file is a PDF (`%PDF-` header), non-trivial size;
  - open the register → both names visible; both total labels visible;
  - download register PDF → PDF;
  - open the payment summary → the "does not mean anyone has been paid"
    sentence visible; "Excluded as reversed: 0" visible;
  - download payment summary PDF → PDF.
- **Reconcile:** the net pay total on the register's "still live" row equals
  the payment summary's total, and equals the sum of the two members' net pay
  shown on their finalized payroll screens.
- Reversal is not reachable from the browser yet (spec Slice 8, a later
  issue), so the reversed/replacement cases stay proven at the HTTP seam
  (steps 2–4). Say that in a comment at the top of the spec.

Run the suite with a fresh `salt_e2e` database, per `AGENTS.md`.

### 2. No-CSV guard

Add one test in `crates/salt-server/tests/router.rs` (or wherever route
listing is easiest) that proves no route answers with `text/csv` and no
route path ends in `.csv`: request `…/register.csv`,
`…/payment-summary.csv` and `…/payslips.csv` for a real finalized run and
assert 404. Add a one-line comment naming D37 as the reason.

Also grep the repo and confirm nothing else produces CSV:
`grep -rniE 'text/csv|\.csv' crates web/src e2e/tests` → only the guard test.

### 3. Docs

- `CONTEXT.md`: under `PayrollRegister`, add the two-totals rule; under
  `PaymentSummary`, the live-only rule, the excluded count, and "an
  instruction to a person, never a bank file (D37)". Keep it short.
- `docs/adr/0021-...md`: one sentence that register and payment summary PDFs
  are also rendered on demand and never stored, but are views, not
  versioned documents, so they carry no template version.
- `docs/domain/payroll-run-persistence.md`: only if it lists read models —
  add the two new ones. Do not rewrite sections.

## Done when

- e2e suite passes (both specs). Rust verification passes.
- Commit: `Prove run outputs end to end for #83`.
- Tick step 6 in `progress.txt`.
