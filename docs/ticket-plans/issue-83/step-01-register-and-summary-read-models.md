# Step 1 — Register and payment summary read models (`payroll-app`)

Read `README.md` first.

## Goal

Two pure read use cases in `payroll-app` that return plain structs. No HTTP,
no PDF, no web.

## Read these files (and only these, unless blocked)

- `crates/payroll-app/src/payslip.rs` — the query shape, liveness/reversal/
  replacement joins, `parse_*_id`, and the snapshot decode helpers.
- `crates/payroll-app/src/finalized_payroll_read.rs` lines 1–320 — the
  frozen-name-first rule, `calculation_from_snapshot`,
  `particulars_from_snapshot`, `FinalizedPayrollDetail`.
- `crates/payroll-app/src/payroll_run.rs` — `PayrollFigures` (~1430),
  `RunStatus`, `RunKind`, `verify_payroll_run_belongs_to_employer` (~1314),
  `get_payroll_run_detail` (~1597, for member order and run row query).
- `crates/payroll-app/src/error.rs` — `PayrollRunNotFound`,
  `PayrollRunAlreadyFinalized` (as a pattern for a new variant).
- `crates/payroll-app/src/lib.rs` — the `pub use` list.
- `crates/payroll-app/tests/payslip_data.rs` — test setup helpers to copy
  (finalizing a run, reversing, creating a correction run).
- `crates/payroll-app/migrations/0010_live_finalized_payroll.sql`,
  `0011_reversal.sql`.

## Build

New module `crates/payroll-app/src/run_outputs.rs`, exported from `lib.rs`.

### Error

Add `PayrollAppError::PayrollRunNotFinalized(PayrollRunId)` with a clear
`Display` ("PayrollRun … has no outputs: it is not finalized"). Every output
use case (this step and step 3) refuses a non-Finalized run with it. Step 2
maps it to HTTP.

### Types

```rust
pub enum FinalizedPayrollLiveness {
    Live,
    Reversed { reason: String, reversed_at: DateTime<Utc>,
               replaced_by: Option<FinalizedPayrollId> },
}

pub struct PayrollRegisterRow {
    pub finalized_payroll_id: FinalizedPayrollId,
    pub employment_id: EmploymentId,
    pub full_name: String,          // frozen first, live fallback (README)
    pub figures: PayrollFigures,    // from the frozen calculation only
    pub liveness: FinalizedPayrollLiveness,
    pub replaces: Option<FinalizedPayrollId>,
}

pub struct PayrollRegisterTotals { /* same money fields as PayrollFigures
    that make sense to sum: basic_pay, taxable_allowances, overtime, gross,
    taxable_remuneration, paye, employee_social_security,
    employer_social_security, medical_aid_premium, total_deductions,
    net_pay */ }

pub struct PayrollRegister {
    pub payroll_run_id: PayrollRunId,
    pub kind: RunKind,              // Ordinary / Correction
    pub period: PayPeriod,
    pub pay_date: NaiveDate,
    pub rows: Vec<PayrollRegisterRow>,
    pub total_as_finalized: PayrollRegisterTotals,  // every row
    pub total_still_live: PayrollRegisterTotals,    // Live rows only
}

pub struct PaymentSummaryRow {
    pub finalized_payroll_id: FinalizedPayrollId,
    pub employment_id: EmploymentId,
    pub full_name: String,
    pub net_pay: Money,
    /// Some when this row is a Replacement. The UI/PDF must then say the
    /// original may already have been paid and a human decides the transfer.
    pub replaces: Option<FinalizedPayrollId>,
}

pub struct PaymentSummary {
    pub payroll_run_id: PayrollRunId,
    pub kind: RunKind,
    pub period: PayPeriod,
    pub pay_date: NaiveDate,
    pub rows: Vec<PaymentSummaryRow>,      // Live rows only
    pub excluded_reversed_count: usize,
    pub total_net_pay: Money,              // sum of rows
}
```

Derive `Debug, Clone, PartialEq, Eq`. If `RunKind` is not public yet, make it
public and export it (check `lib.rs` first). Put a totals-summing helper next
to `PayrollRegisterTotals`; use `Money`'s own checked/`+` API (look at
`crates/payroll/src/money.rs`), never `f64`.

### Use cases

```rust
pub async fn get_payroll_register(db, employer_id, payroll_run_id: &str)
    -> Result<PayrollRegister, PayrollAppError>;
pub async fn get_payment_summary(db, employer_id, payroll_run_id: &str)
    -> Result<PaymentSummary, PayrollAppError>;
```

- One read transaction (like `get_payroll_run_detail`).
- Run row scoped by `employer_id`; missing → `PayrollRunNotFound`.
- Status not `Finalized` → `PayrollRunNotFinalized`.
- One query for rows: `finalized_payroll` of this run + `person` (for the
  fallback name) + `live_finalized_payroll` + `reversal` + replacement join,
  `ORDER BY finalized_payroll.employment_id`.
- `get_payment_summary` builds from the same private row reader as the
  register (one private fn, two public projections). Do not write two SQL
  queries.
- A row with **no** frozen particulars (pre-#73) must work — it only needs
  the calculation and a name.

## Tests — new file `crates/payroll-app/tests/run_outputs.rs`

Copy helpers from `payslip_data.rs`. Cover:

1. Two finalized members: register has both rows in employment-id order;
   both totals equal the sum of the rows; summary has both, excluded 0.
2. After reversing one row: register still shows it as `Reversed` with reason;
   `total_as_finalized` unchanged; `total_still_live` drops by exactly that
   row; summary excludes it and `excluded_reversed_count == 1`.
3. Reverse then create a correction run and finalize its replacement:
   - original run's register row names `replaced_by`;
   - correction run's register has exactly one row with `replaces` set;
   - correction run's summary shows the replacement's **full** net pay with
     `replaces` set.
4. A row with no frozen particulars (finalize without setting employer
   particulars, as `payslip_data.rs` does for its refusal test): register and
   summary still succeed; name comes from the live person.
5. Frozen name wins: correct the person's name after finalization; register
   still shows the old frozen name.
6. A Draft/Calculated run → `PayrollRunNotFinalized`.
7. Another employer's run id and a random uuid → both `PayrollRunNotFound`.
8. Figures come from the snapshot: assert a register row's figures equal
   `get_finalized_payroll_detail(...).figures` for the same id.

## Done when

- Verification in `README.md` passes.
- Commit: `Add register and payment summary read models for #83`.
- Tick step 1 in `progress.txt`.
