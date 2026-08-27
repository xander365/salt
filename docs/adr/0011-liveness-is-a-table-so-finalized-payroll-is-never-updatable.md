# Liveness is a separate table, so finalized payroll is never updatable

Which `FinalizedPayroll` currently counts for an Employment and a PayPeriod is held in its own table, `live_finalized_payroll`, keyed `(employment_id, period_end)` and pointing at one finalized row. Finalizing inserts; a `Reversal` deletes; a replacement inserts again. We rejected the obvious alternative: a nullable `reversed_by` column on `finalized_payroll` with a partial unique index `WHERE reversed_by IS NULL`.

The rejected design is the one a reader will reach for, so the reason has to be written down. It requires the application role to hold `UPDATE` on the table whose entire purpose is that it is never updated. INV-004 would then be a convention enforced by whoever writes the next query. With liveness held separately, Salt can `REVOKE UPDATE, DELETE ON finalized_payroll` outright, and immutability becomes a database permission — the same move ADR-0008 made when it turned "someone will notice in review" into a failing test.

The single primary key also does two jobs at once. It makes duplicate initial finalization impossible — two processes that both believe finalization is allowed cannot both insert — and it makes duplicate *replacement* impossible, which a naive `UNIQUE (employment_id, period)` on the history table cannot do, because a reversed original and a live replacement must both exist there forever.

The deleted liveness row is not deleted history. History is the `FinalizedPayroll` row, which is untouched, and the `Reversal` row, which records who cancelled it, when, and why. `live_finalized_payroll` is an index of current truth, not a record of what happened. ADR-0006's "Salt ships no deletion of payroll history" is unaffected.

## Consequences

- Year-to-date joins through `live_finalized_payroll` and **never mentions reversals**. A reversed period is absent because its liveness row is gone; a replacement is present because it inserted a new one.
- Concurrency safety does not rest on transaction isolation level or on an application-side `if status != Finalized`. `READ COMMITTED` plus the `SELECT … FOR UPDATE` lock on the `PayrollRun` row plus this primary key is the whole mechanism.
- A replacement lives in a new `PayrollRun` with `kind = Correction`, because the original run is finalized and reopening it would contradict ADR-0010. Uniqueness of "one run per Employer + PayPeriod" therefore applies to `Ordinary` runs only.
- Replacement lineage is an explicit nullable `replaces_finalized_payroll_id`, set at insert and never updated. Inferring the chain from `(employment_id, period_end)` ordered by `finalized_at` works until a period is corrected twice.
- A reversal cannot be undone. A mistaken reversal is corrected by finalizing a replacement identical to the original — the live set already yields the right arithmetic, and an "unreversal" would be a fourth history shape built for one rare mistake.
- The design owes a test that an `UPDATE` on `finalized_payroll` is refused **by the database role itself**, not by application code. Without it the `REVOKE` can quietly go missing in a future migration.
