# Step 7a — Code-review findings and fix plan

Read `README.md` and `step-07-review-and-harden.md` first. This document is
the handoff from the Step 7a review; it is not a second specification.

## Review scope and baseline

- Reviewed range: `31c6c3527c861d3bf087795a94a666c933688804` through
  `305a07729ff943aaf59f5d23f2792e85eefa8024`.
- Requirements: issue #83, issue #70 §D-9/§D-10, and this directory's
  `README.md` plus Steps 1–6.
- Preserve the existing uncommitted edit to
  `step-07-review-and-harden.md` and the untracked `.playwright-mcp/`
  directory. They are outside the reviewed range and belong to the user.
- The work below fits in one 150k context window. Complete it as one
  sequential pass; no further plan split is needed.

The review baseline passed:

```text
cargo build --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --check
DATABASE_URL="postgres:///salt_server_test?host=/var/run/postgresql&user=alex" \
  cargo test --workspace --locked --no-fail-fast
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked
npm --prefix web run build
npm --prefix web run lint
```

The Step 7a review did not rerun Playwright. Run it after the browser-test
isolation fix, using the fresh-database procedure in `AGENTS.md`.

## Step 1 — Restore required register-PDF status content

### Finding

`crates/salt-server/src/run_outputs_render.rs::status_word` prints only
`Reversed`. Step 4 requires the status column itself to print
`Reversed — replaced by <id>` when a reversed row has a replacement. The
current row-numbered note preserves the lineage, but moving required status
content into a note does not satisfy the status-column contract.

### Fix

1. Replace `status_word` with a function that can return owned text:
   `Live`, `Reversed`, or `Reversed — replaced by <id>`.
2. Pass that complete value to `draw_table_row`; keep the row-numbered note
   because it also carries the reversal reason.
3. Let the existing status-column wrapping handle the UUID. Preserve the
   page-margin and no-overlap guarantees.
4. Strengthen the renderer test so it proves the replacement id is in the
   status cell, not merely somewhere in the document. Use text placements or
   a small unit test around the status-text function.

### Complete when

- A reversed-and-replaced row's status value is exactly
  `Reversed — replaced by <id>`.
- The reason remains in the numbered notes.
- The 50-row placement test remains green.

## Step 2 — Keep zero totals visible in empty output states

### Finding

- `web/src/routes/PaymentSummary.tsx` replaces the whole table with an
  `EmptyState` when `rows.length === 0`. An all-reversed run therefore hides
  the server-provided `Total net pay` of N$0.00.
- `web/src/routes/PayrollRegister.tsx` likewise hides both required zero
  total lines for a finalized run with no rows. Vacuous finalization is a
  supported state and already has an integration test.

### Fix

1. Render `EmptyState` as an explanation in addition to, rather than instead
   of, the totals.
2. Keep the semantic table and its `<tfoot>` visible for zero rows; an empty
   `<tbody>` is valid.
3. Preserve `Excluded as reversed: N` outside the PaymentSummary table.
4. Add focused component/browser coverage for:
   - an all-reversed PaymentSummary: no live body rows, N$0.00 total, and the
     full excluded count;
   - an empty PayrollRegister: both total labels and zero figures.
   If browser setup cannot create reversals yet, prove the React branches at
   the nearest existing frontend test seam and retain the server tests from
   Step 7b.

### Complete when

- Empty states explain the absence of rows without suppressing totals.
- The all-reversed summary visibly says N$0.00.
- The empty register visibly shows both required total labels.

## Step 3 — Make the outputs browser spec self-contained

### Finding

`e2e/playwright.config.ts` makes `outputs.spec.ts` depend on the `journey`
project, and `outputs.spec.ts` consumes the run that
`payroll-journey.spec.ts` created. This conflicts with Step 6 and issue #70's
focused-spec decision: the outputs spec must set up its own two-Employment
run, shared setup should be extracted, and the original journey's behaviour
must not be expanded merely to seed another test.

### Fix

1. Extract only the reusable UI setup operations into
   `e2e/tests/helpers/`; keep assertions in their owning specs.
2. Restore `payroll-journey.spec.ts` to its pre-Step-6 scenario unless a
   changed line is independently required by that journey.
3. Have `outputs.spec.ts` create two Employments, declare their facts, create
   a run, calculate it, and finalize it through the UI before testing
   outputs.
4. Choose fixture names and a PayPeriod that let the spec run both:
   - by itself against a fresh database; and
   - in the complete suite without colliding with another Ordinary run.
5. Remove the Playwright project dependency. Keep one Chromium browser and
   one worker unless the now-independent setup proves parallel-safe.
6. Run the full e2e suite with a fresh database as prescribed by `AGENTS.md`.
   Then rerun only `outputs.spec.ts` against another fresh database to prove
   it has no hidden dependency.

### Complete when

- `outputs.spec.ts` reaches its finalized run solely from setup it performs
  or calls directly.
- No test relies on a previous test's database mutations.
- Both the complete suite and the isolated outputs spec pass from fresh
  databases.

## Step 4 — Add an explicit no-bank-file regression guard

### Finding

`run_outputs_have_no_csv_routes` requests only `.csv` paths and checks only
for `text/csv`. Its comment mentions bank files, but no assertion proves the
deliberate absence required by acceptance criterion 7.

### Fix

1. Keep the existing three `.csv` 404 assertions.
2. Add a criterion-specific inspection of the production route table that
   fails if an output route contains bank/payment-file terminology or an
   export route. Reuse the existing router-source parsing approach rather
   than maintaining a second hand-written route inventory.
3. Keep the assertions scoped to HTTP/product vocabulary; Rust/TypeScript's
   `export` keyword is not product wording.
4. Retain the repository grep from Step 6 as review evidence, not as the sole
   automated guard.

### Complete when

- One named test fails if a CSV or bank/payment-file output route is added.
- The three current `.csv` paths still return 404 and never `text/csv`.

## Step 5 — Remove the duplicated frontend error predicates

### Finding

`runWasNotFound` and `runNotFinalized` are identical in
`PayrollRegister.tsx` and `PaymentSummary.tsx` (possible Duplicated Code;
heuristic, not a documented hard violation).

### Fix

Move the two predicates into one small module under `web/src/runOutputs/`
and import them from both screens. Keep screen-specific failure messages in
their screens.

### Complete when

- Each predicate has one implementation.
- Both screens retain the current not-found, not-finalized, and retry paths.

## Step 6 — Repair progress metadata

### Finding

`progress.txt` says every checked step carries its commit hash, but Step 6
was checked without `305a077`.

### Fix

Append `305a077` to Step 6. When the review-fix commit exists, replace Step
7a's temporary handoff marker with that commit hash.

### Complete when

Every checked implementation step names a resolvable commit.

## Step 7 — Verify and hand off

1. Format changed web files with the command in `README.md`.
2. Run the full Rust and web verification listed in `README.md`.
3. Run the full e2e suite and the isolated outputs spec against separate
   fresh databases.
4. Confirm `git diff --check` is clean.
5. Record the acceptance mapping below in the review-fix commit body, using
   updated test names if any were renamed.

### Acceptance mapping

1. Batch Payslip PDF →
   `two_members_print_in_member_order_each_starting_a_new_page` and
   `a finalized run renders and reconciles every output`.
2. Register rows, liveness and two totals →
   `two_finalized_members_appear_in_the_register_and_summary` and
   `reversing_a_row_keeps_it_in_the_register_but_drops_it_from_the_summary`;
   add the empty-register zero-total test from Step 2.
3. Live-only PaymentSummary, exclusion count and nobody-paid warning →
   `reversing_a_row_keeps_it_in_the_register_but_drops_it_from_the_summary`,
   `a_reversal_splits_the_register_totals_and_shrinks_the_payment_summary`,
   and `the_summary_always_states_nobody_has_been_paid_and_the_excluded_count`;
   add the all-reversed zero-total test from Step 2.
4. Replacement full net pay and warning →
   `a_replacement_row_carries_full_net_pay_and_replaces` and
   `the_replacement_note_prints_only_for_a_row_that_replaces_another`.
5. On-screen and PDF forms →
   `a finalized run renders and reconciles every output` plus
   `register_and_payment_summary_pdfs_download_as_uncacheable_pdfs`.
6. Register and summary without frozen particulars →
   `both_routes_answer_200_without_frozen_particulars` and
   `both_pdf_routes_answer_200_without_frozen_particulars`.
7. No CSV or bank file → `run_outputs_have_no_csv_routes` plus the expanded
   bank/payment-file route guard from Step 4.
8. Cross-Employer ids match missing ids →
   `a_cross_employer_run_id_is_not_found_like_an_unknown_one_on_both_routes`,
   `a_cross_employer_run_id_is_not_found_like_an_unknown_one_on_both_pdf_routes`,
   `a_run_id_belonging_to_another_employer_or_unknown_is_not_found`, and
   `a_finalized_payroll_id_belonging_to_another_employer_is_not_found`.

### Final completion criterion

All six findings above are fixed, every added regression test is red before
its fix where practical and green afterward, full verification and both e2e
runs pass, and the commit body maps all eight criteria to tests.
