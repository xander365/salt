# Step 4 — Register and payment summary PDFs (`salt-server`)

Read `README.md` first. Steps 1 and 2 must be done. Step 3 is helpful (its
refactor of `payslip_render.rs` shows how to share layout code).

## Goal

```
GET /api/employers/{e}/payroll-runs/{r}/register.pdf
GET /api/employers/{e}/payroll-runs/{r}/payment-summary.pdf
```

## Read these files

- `crates/payroll-app/src/run_outputs.rs` (step 1).
- `crates/salt-server/src/run_outputs.rs` (step 2).
- `crates/salt-server/src/payslip_render.rs` lines 170–560 — `Layout`,
  `row`, `total_row`, `heading`, `format_money`, `format_instant`, fonts,
  page constants, and the test parser at the end of the file.
- `crates/salt-server/src/payslip.rs` — `pdf_response`.

## Build

- Move the reusable drawing pieces (`Layout`, font loading, `format_money`,
  `format_instant`, `row`, `heading`, text wrap, test parser) into a new
  module `crates/salt-server/src/pdf_layout.rs` **only if** step 3 did not
  already make them shareable. Keep behaviour identical; payslip tests must
  still pass.
- New module `crates/salt-server/src/run_outputs_render.rs`:
  - `pub struct RegisterPdfInput` / `PaymentSummaryPdfInput` — plain data
    (strings and cents), built in `run_outputs.rs` from the step-1 structs.
    No `payroll_app` types inside the renderer, same seam as `PayslipInput`.
  - `pub fn render_register(&RegisterPdfInput) -> Vec<u8>`
  - `pub fn render_payment_summary(&PaymentSummaryPdfInput) -> Vec<u8>`
  - These are **not** versioned templates: they are views, not issued
    statutory documents, and they need no frozen particulars. Say so in the
    module doc.
  - A4 **landscape** for the register (many columns); portrait for the
    summary. Right-align money. Wrap long names.
  - A generated-at line is fine; the content under test must not depend on it.

### Register PDF must print

- Title "Payroll register", employer registered name **only if** the step-1
  data carries it — it does not; so print the run period, pay date and run
  kind instead. Do not read current employer particulars.
- One row per `PayrollRegisterRow`: name, basic pay, allowances, overtime,
  gross, PAYE, employee SSC, medical aid, total deductions, net pay,
  employer SSC, and a **status** column: `Live`, `Reversed`, or
  `Reversed — replaced by <id>`; a replacement row also says
  `Replaces <id>`. Status is words, never colour only.
- Two total lines, labelled exactly **"Total as finalized by this run"** and
  **"Total still live from this run"**.
- A reversed row's reason printed under the table (row-numbered notes).

### Payment summary PDF must print

- Title "Payment summary", period, pay date.
- Live rows only: name and net pay; total net pay.
- "Excluded as reversed: N" — always printed, even when N is 0.
- Always, near the top, in body size, not small print:
  **"This is an instruction to a person. Producing it does not mean anyone
  has been paid, and finalizing payroll does not mean a bank transfer
  happened. This is not a bank file."**
- For every replacement row, a note:
  **"This replaces an earlier payroll for the same period. The original may
  already have been paid. A person must decide the actual transfer."**
- For a correction run, one more sentence: reversing a payroll in Salt does
  not recover money already paid and does not amend a submitted statutory
  return.

Handlers in `crates/salt-server/src/run_outputs.rs`; filenames
`register-<run-id>.pdf`, `payment-summary-<run-id>.pdf`; `pdf_response`
(no-store). Routes in `router.rs`.

## Tests

Unit tests in `run_outputs_render.rs` (use `rendered_text`):

- both total labels present with the right figures;
- reversed row says `Reversed`, and reason appears;
- summary always contains the "does not mean anyone has been paid" sentence
  and "Excluded as reversed: 0";
- replacement note present only when a row replaces another;
- a 50-row register paginates without text overlap (use `text_placements`,
  same style as the payslip layout tests).

HTTP tests appended to `crates/salt-server/tests/run_outputs.rs`:

- both PDFs: 200, `application/pdf`, `no-store`;
- figures printed equal the JSON route's figures (content comparison);
- pre-#73 style row: both PDFs 200;
- not finalized → 409; isolation → identical 404s; no session → 401.

## Done when

- Verification in `README.md` passes.
- Commit: `Render register and payment summary PDFs for #83`.
- Tick step 4 in `progress.txt`.
