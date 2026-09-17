# Step 5 — Outputs on the finalized run screen (`web`)

Read `README.md` first. Steps 2, 3 and 4 must be done.

**Invoke the `/impeccable` skill before writing UI** (repo `CLAUDE.md`).
Follow `DESIGN.md` (tokens only, no hard-coded colours; money right-aligned,
tabular numerals, N$; states via `web/src/components/states/*`).

## Goal

On a **Finalized** `PayrollRun` screen, an "Outputs" section with:

- **Download all payslips (PDF)** → `payslips.pdf`.
- **Payroll register** → an on-screen view, with **Download PDF**.
- **Payment summary** → an on-screen view, with **Download PDF**.

No Reports tab. No nav change. No CSV. Not shown on a non-finalized run.

## Read these files

- `web/src/routes/PayrollRun.tsx` (866 lines — read the finalized branch and
  where `PayslipDownload` is used).
- `web/src/finalizedPayroll/PayslipDownload.tsx`,
  `web/src/finalizedPayroll/usePayslipDownload.ts` — copy this download
  pattern (loading, failure, retry).
- `web/src/api/client.ts`, `web/src/api/types.ts`, `web/src/api/refusal.ts`.
- `web/src/routes/paths.ts`, `web/src/App.tsx` (route table).
- `web/src/routes/payroll/Figures.tsx`, `web/src/components/Money.tsx`.
- `web/src/components/states/*` (loading, empty, failed-request, finalized).
- `web/src/routes/payroll/refusalText.ts` — branch on `code`, never prose.

## Build

- `api/types.ts`: `PayrollRegister`, `PaymentSummary` types matching step 2's
  DTOs exactly.
- `api/client.ts`: `getPayrollRegister`, `getPaymentSummary`, and a generic
  PDF download helper for the three run PDFs (reuse the payslip one; make it
  take a URL and filename if it does not already).
- Hooks in a new folder `web/src/runOutputs/`:
  `usePayrollRegister.ts`, `usePaymentSummary.ts` (React Query, keys include
  employerId so employer switching clears them), `useRunPdfDownload.ts`.
- Routes (nested under the employer shell, beside `payroll/:runId`):
  - `payroll/:runId/register` → `web/src/routes/PayrollRegister.tsx`
  - `payroll/:runId/payment-summary` → `web/src/routes/PaymentSummary.tsx`
  - Add path builders in `paths.ts`.
  - Each page has a back link to the run.
- `PayrollRun.tsx`: an `RunOutputs` component (in `web/src/runOutputs/`)
  rendered only when the run is Finalized.

### Register screen

- Table: name, basic pay, allowances, overtime, gross, PAYE, employee SSC,
  medical aid, total deductions, net pay, employer SSC, status.
- Status as text + badge: `Live` / `Reversed` / `Reversed — replaced by …`
  (link to the replacement's `finalizedPayrollPath`); `Replaces …` link for a
  replacement. Never colour alone.
- Each row's name links to its finalized payroll screen.
- Two footer rows labelled **"Total as finalized by this run"** and
  **"Total still live from this run"**, plus one sentence explaining the
  difference (a reversal is shown, never hidden, never counted twice).
- Dense and readable at 20 and 50 rows.

### Payment summary screen

- The "nobody has been paid / not a bank file" sentence, visible in the
  page body (not a tooltip), above the table. Same words as the PDF.
- Table: name, net pay; total row.
- "Excluded as reversed: N" always shown.
- Replacement rows: an inline note with the "original may already have been
  paid; a person decides the transfer" sentence.
- For a correction run: the sentence that a reversal in Salt does not recover
  money and does not amend a submitted return.

### States

- Loading → `LoadingState`. Failed request → `FailedRequestState` with retry.
- `payroll_run_not_finalized` → a sentence and a link back to the run.
- Batch payslip refusal `payslip_particulars_not_frozen` → a sentence saying
  this run was finalized before Salt recorded payslip particulars, and that
  the register and payment summary still work (link to them).
- Empty run → `EmptyState`.

## Checks

- `npx prettier --single-quote --print-width 100 --write` on changed files.
- `npm --prefix web run build` and `npm --prefix web run lint` clean.
- Launch the app (`/run` skill or `AGENTS.md`) and look at both screens at
  laptop and narrow width. Fix overflow.

## Done when

- Checks pass and the Rust verification still passes (no Rust change
  expected).
- Commit: `Show run outputs on the finalized run screen for #83`.
- Tick step 5 in `progress.txt`.
