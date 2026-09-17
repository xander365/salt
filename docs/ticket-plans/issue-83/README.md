# Issue #83 — Batch payslips, payroll register and payment summary

Parent spec: #70 (Slice 7, §D-9, §D-10). Blocked by #82 (done: `10c6085`, `6dfbb79`, `31c6c35`).

Read this file first in every step. Then read **only** your step file. Tick
progress in `progress.txt` when a step is done.

## What #83 asks for (acceptance criteria, verbatim intent)

1. An operator downloads **every payslip in a run as one PDF**.
2. The **PayrollRegister** shows every Employment in the run with its figures,
   each row's **liveness**, and **two totals**: "Total as finalized by this
   run" and "Total still live from this run".
3. The **PaymentSummary** shows names and net pay for **live records only**,
   states **how many rows it excluded as reversed**, and says plainly that
   **producing it means nobody has been paid**.
4. A **Replacement's** payment summary row shows **full net pay** (not a
   difference) and states that **the original may already have been paid**, so
   a human must decide the actual transfer.
5. Register and payment summary render **on screen and as PDF**.
6. Both still work for a payroll whose **particulars never froze** (they need
   only figures).
7. **No CSV and no bank file** anywhere in the product.
8. Every new route refuses a **cross-Employer id exactly like a missing one**
   (ADR-0017).

## Rules that apply to every step

- Register and PaymentSummary are **views over `finalized_payroll` rows**.
  Read figures from the frozen `payroll_calculation_json` via
  `PayrollFigures::from_calculation`. **Never recompute. Never read current
  master data for a figure.**
- Liveness: a `FinalizedPayroll` is **live** when a `live_finalized_payroll`
  row names its id (migration 0010). It is **reversed** when a `reversal` row
  names it (migration 0011). It is a **replacement** when
  `finalized_payroll.replaces_finalized_payroll_id` is set. A reversed row may
  also have a replacement (`finalized_payroll AS replacement ON
  replacement.replaces_finalized_payroll_id = finalized_payroll.id`). Copy the
  joins from `crates/payroll-app/src/payslip.rs::get_payslip_data`.
- **Names:** prefer the frozen `person_particulars_json.full_name`; fall back
  to the live `person.full_name` only for a row that froze none. This is the
  exact rule `get_finalized_payroll_detail` already follows
  (`crates/payroll-app/src/finalized_payroll_read.rs`, around line 180).
  Reuse it; do not invent a second rule.
- **Member order:** `ORDER BY employment.id` — the order
  `get_payroll_run_detail` uses (`crates/payroll-app/src/payroll_run.rs`
  ~line 1680). Batch payslips, register and summary all use this one order.
- **Run scope:** only rows with `finalized_payroll.payroll_run_id = <run>` —
  that run's own records, including a CorrectionRun's single member. A
  reversal of this run's row made later is shown; a replacement made by a
  **different** (correction) run is named but is **not** a row of this run.
- A run that is **not Finalized** has no outputs. No error for this exists
  today, so step 1 adds `PayrollAppError::PayrollRunNotFinalized` and step 2
  maps it to **409 `payroll_run_not_finalized`**.
- Employer scope in SQL (`WHERE ... employer_id = $2`) and via
  `verify_payroll_run_belongs_to_employer`. Unknown and cross-Employer run ids
  both answer `PayrollRunNotFound` → 404.
- PDFs: `Content-Type: application/pdf`, `Cache-Control: no-store`, built by
  the existing `pdf_response` helper in `crates/salt-server/src/payslip.rs`.
  Never stored (ADR-0021).
- **No CSV, no bank file, no "export" wording.** No Reports tab (D34).
- `salt-server` writes no SQL (ADR-0018). All SQL goes in `payroll-app`.
- Test names: `algorithm_*` / `salt_policy_*` only where the existing
  taxonomy asks; plain descriptive names are fine for read models and routes.
  Never `statutory_*`.
- Frontend work: invoke the `/impeccable` skill (repo `CLAUDE.md`). Format web
  code with `npx prettier --single-quote --print-width 100 --write <files>`.

## Verification (run before every commit)

```sh
cargo build --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --check
DATABASE_URL="postgres:///salt_server_test?host=/var/run/postgresql&user=alex" \
  cargo test --workspace --locked --no-fail-fast
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
```

Web steps also: `npm --prefix web run build` and `npm --prefix web run lint`.
The e2e command is in `AGENTS.md` ("Browser journey suite").

No new migration is expected in this issue. If one appears necessary, stop and
ask — the spec's migration list ends at 0037 and #83 needs no schema change.

## Steps

| # | File | Crates / area | Commit message |
|---|---|---|---|
| 1 | `step-01-register-and-summary-read-models.md` | payroll-app | `Add register and payment summary read models for #83` |
| 2 | `step-02-register-and-summary-json-routes.md` | salt-server | `Serve register and payment summary JSON for #83` |
| 3 | `step-03-batch-payslips-pdf.md` | payroll-app + salt-server | `Render every payslip in a run as one PDF for #83` |
| 4 | `step-04-register-and-summary-pdfs.md` | salt-server | `Render register and payment summary PDFs for #83` |
| 5 | `step-05-web-outputs-on-finalized-run.md` | web | `Show run outputs on the finalized run screen for #83` |
| 6 | `step-06-e2e-docs-and-no-csv-guard.md` | e2e, docs, guard test | `Prove run outputs end to end for #83` |
| 7 | `step-07-review-and-harden.md` | all | `Act on the code review of #83`, then `Harden the run outputs of #83` |

Steps 1 → 2 → 4 are a chain. Step 3 needs only step 1's conventions (it can
run after 1). Step 5 needs 2, 3 and 4. Step 6 needs 5.
