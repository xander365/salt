# Year-to-date sums real columns, never the frozen snapshots

`FinalizedPayroll` freezes the `PayrollInput`, the resolved `PayrollRules` and the `PayrollCalculation` as JSONB, and **additionally** stores `taxable_remuneration` and `paye` as ordinary numeric columns. Year-to-date reconstruction sums the columns. We rejected extracting those two figures from the JSONB snapshot, which would have avoided the duplication.

The duplication looks like a normalisation smell, and it is deliberate. ADR-0004 requires history to be *explainable*, not *re-runnable* — Salt keeps the raw data forever, but it never promised that every old snapshot deserializes into the newest Rust structs forever. Those are different obligations, and summing JSON paths quietly binds them together: an ordinary field rename in `PayrollCalculation` would stop being a refactor and start being an arithmetic break across every prior period of every employee. Cumulative PAYE (ADR-0001) means that break is not a display bug — it is withholding the wrong amount.

Columns sever that link. Year-to-date stays correct whether or not a ten-year-old snapshot still parses. The two representations cannot drift, because they are written by one `INSERT` from one value into a row that has no `UPDATE` grant (ADR-0011).

`snapshot_schema_version` ships on the same row from day one, as a small integer. `SaltVersion` alone is technically sufficient — every version maps to some shape — but the recovery is a maintained lookup table from every Salt release to a snapshot layout, built years later by someone who was not here. An integer states it directly, and it is what a reader branches on to render old history without constructing current domain types.

## Consequences

- **There are no in-place JSON migrations, ever.** This is forced rather than chosen: the application has no `UPDATE` grant on `finalized_payroll`. A shape change means new rows carry version 2 and readers branch on the version.
- `PeriodsElapsed` is not one of the duplicated figures and must never be derived by counting rows. It is the PayPeriod's position in its TaxYear, derived in the pure crate. Counting finalized rows over-withholds from every mid-year joiner — the failure `year_to_date.rs` documents at length.
- `OpeningBalance` does not store `periods_elapsed` either, for the same reason.
- Ordering is by `PayPeriod.end_date`, never `finalized_at`. A late-finalized March is still March, and a replacement finalized in October is still March's figure.
- The design owes a test that a later `CompensationTerms` or Employment change cannot alter a frozen figure, and one that year-to-date orders by pay period rather than finalization timestamp.
