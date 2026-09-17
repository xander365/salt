# Step 3 — Every payslip in a run as one PDF

Read `README.md` first. Step 1 must be done (for `PayrollRunNotFinalized`).

## Goal

```
GET /api/employers/{e}/payroll-runs/{r}/payslips.pdf
```

One PDF. One payslip after another, each starting on a new page, in member
order (`employment.id`). Covers **that run's own** FinalizedPayrolls only,
including reversed ones (each still marked REVERSED, as the single payslip
already does) and a CorrectionRun's single member.

## Read these files

- `crates/payroll-app/src/payslip.rs` — `get_payslip_data`.
- `crates/salt-server/src/payslip.rs` — handler, `pdf_response`,
  `to_render_input`.
- `crates/salt-server/src/payslip_render.rs` lines 1–720 — `render_payslip`,
  `render_standard_v1`, the `Layout` type, pagination. The rest of that file
  is the test PDF parser (`text_placements`, `rendered_text`); use it, do not
  rewrite it.
- `docs/adr/0021-a-payslip-is-rendered-on-demand-and-never-stored.md`.
- `crates/salt-server/tests/payslip.rs` — helpers and assertion style.

## Build

### payroll-app

- In `payslip.rs`, split the body of `get_payslip_data` so the row→
  `PayslipData` conversion is a private fn. Add:
  ```rust
  pub async fn get_run_payslip_data(db, employer_id, payroll_run_id: &str)
      -> Result<Vec<PayslipData>, PayrollAppError>;
  ```
  Same SELECT, but `WHERE finalized_payroll.payroll_run_id = $1::uuid AND
  finalized_payroll.employer_id = $2 ORDER BY finalized_payroll.employment_id`.
  Check the run first (not found → `PayrollRunNotFound`; not finalized →
  `PayrollRunNotFinalized`).
- **Missing particulars in a batch: refuse the whole batch.** Return
  `PayslipParticularsNotFrozen` for the **first** row (in member order) that
  lacks them. Reason: a batch with silent holes is worse than a clear
  refusal, and the register and summary still work for that run. Keep the
  existing error type; do not add a new one.
- Export the new fn from `lib.rs`.

### salt-server

- In `payslip_render.rs`, add
  `pub fn render_payslips(inputs: &[PayslipInput]) -> Result<Vec<u8>, PayslipRenderError>`.
  Refactor so each template version draws **into a shared document** starting
  on a fresh page; `render_payslip` becomes `render_payslips(&[input])` or
  shares the same inner drawing fn. **The single-payslip visible content must
  not change** — existing tests in `tests/payslip.rs` and the unit tests at
  the bottom of `payslip_render.rs` must pass unchanged.
- Each input still dispatches on its **own** `payslip_template_version`
  (ADR-0021). Unknown version → the existing error.
- An empty run (no finalized rows) → return a valid PDF with one page that
  says no payslips were finalized in this run. Do not 404.
- Handler `get_payroll_run_payslips` in `payslip.rs`; filename
  `payslips-<run-id>.pdf`; use `pdf_response` (no-store). Add the route in
  `router.rs`.

## Tests

`crates/salt-server/tests/payslip.rs` (or a new `run_payslips.rs` if that
file is too big to work in):

1. Two members → 200, `application/pdf`, `no-store`; `rendered_text` contains
   both names; first name's page index < second's (use `text_placements`
   page numbers); the second payslip starts on a new page.
2. Each payslip's extracted text in the batch equals the single-payslip text
   for that id (content, not bytes).
3. A reversed row in the batch still prints its REVERSED marking.
4. A correction run's batch has exactly one payslip, marked REPLACEMENT.
5. A run where one member has no frozen particulars → 409
   `payslip_particulars_not_frozen`.
6. Not finalized → 409 `payroll_run_not_finalized`.
7. Isolation: other Employer's run id and random uuid → same 404.
8. No session → 401; PayrollOperator → 200.

`crates/payroll-app/tests/payslip_data.rs`: `get_run_payslip_data` order,
run scoping (a second run's rows never appear), and the refusal.

## Done when

- Verification in `README.md` passes.
- Commit: `Render every payslip in a run as one PDF for #83`.
- Tick step 3 in `progress.txt`.
