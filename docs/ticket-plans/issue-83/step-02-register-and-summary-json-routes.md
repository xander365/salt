# Step 2 — Register and payment summary JSON routes (`salt-server`)

Read `README.md` first. Step 1 must be done.

## Goal

```
GET /api/employers/{e}/payroll-runs/{r}/register          (JSON)
GET /api/employers/{e}/payroll-runs/{r}/payment-summary   (JSON)
```

## Read these files

- `crates/payroll-app/src/run_outputs.rs` (from step 1).
- `crates/salt-server/src/finalized_payroll.rs` — DTO style (camelCase,
  money shape, `From` impls), handler signature with
  `AuthorizedEmployerContext`.
- `crates/salt-server/src/payroll_runs.rs` — how run handlers take path ids
  and how `PayrollFigures` is already turned into a DTO (reuse that DTO; do
  not create a second figures shape).
- `crates/salt-server/src/router.rs` lines 54–170.
- `crates/salt-server/src/payroll_error.rs` — the error→status/code table and
  its exhaustive mapping test.
- `crates/salt-server/tests/payslip.rs` — HTTP test helpers (login, create
  employment, finalize, reverse via `payroll_app` directly ~line 1001).
- `web/src/api/types.ts` only to see which money/figures names the browser
  already expects (do not edit web in this step).

## Build

- New module `crates/salt-server/src/run_outputs.rs` with the two handlers and
  their response DTOs. Register the module in `lib.rs`.
- Register DTO: `payrollRunId`, `kind` (`"ordinary"`/`"correction"`),
  `period`, `payDate`, `rows[]`, `totalAsFinalized`, `totalStillLive`.
  Row: `finalizedPayrollId`, `employmentId`, `fullName`, `figures`,
  `liveness` as a tagged object:
  `{ "state": "live" }` or
  `{ "state": "reversed", "reason", "reversedAt", "replacedBy": id|null }`,
  and `replaces: id|null`.
- Payment summary DTO: `payrollRunId`, `kind`, `period`, `payDate`, `rows[]`
  (`finalizedPayrollId`, `employmentId`, `fullName`, `netPay`, `replaces`),
  `excludedReversedCount`, `totalNetPay`.
  **Do not put the human sentences in the JSON.** The screen and PDF own
  wording; JSON carries facts.
- Map `PayrollRunNotFinalized` in `payroll_error.rs`: **409**, code
  `payroll_run_not_finalized`. Extend the exhaustive mapping test.
- Add both routes in `router.rs` next to the other `payroll-runs/{id}` routes.
- `Cache-Control: no-store` is not required for JSON; do not add it unless
  the other JSON routes do.

## Tests — new file `crates/salt-server/tests/run_outputs.rs`

1. Operator gets 200 register with two rows and both totals.
2. After a reversal (via `payroll_app::reverse_finalized_payroll`): register
   `totalAsFinalized.netPay` ≠ `totalStillLive.netPay`; the difference is the
   reversed row's net pay; row shows `state: "reversed"`.
3. Payment summary after that reversal: one row fewer,
   `excludedReversedCount: 1`.
4. Replacement row in a correction run's summary has `replaces` set and full
   net pay.
5. Pre-#73 style row (no employer particulars set before finalizing): both
   routes 200, while `payslip.pdf` for the same id is 409
   `payslip_particulars_not_frozen`.
6. Not-finalized run → 409 `payroll_run_not_finalized` on both.
7. **Isolation, per route:** another Employer's run id → 404 identical to a
   random uuid → 404 (compare status and body `code`).
8. No session → 401 on both. PayrollOperator role → 200 on both.

## Done when

- Verification in `README.md` passes.
- Commit: `Serve register and payment summary JSON for #83`.
- Tick step 2 in `progress.txt`.
