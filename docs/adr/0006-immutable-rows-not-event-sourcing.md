# Audit is immutable rows plus an action log, not event sourcing

Payroll history is `FinalizedPayroll` and `Reversal` rows that are never updated, alongside a separate append-only action log recording who did what and when. We rejected event-sourcing the payroll domain.

Finalization already provides immutability, so event sourcing would tax every feature for a benefit we have not needed. Unfinalized calculations are working state: Salt keeps only the latest calculation per Employment per run and overwrites the rest. Salt ships no deletion of payroll history at all — the Labour Act and Social Security Act require five years, and "we kept it" is never the thing that gets an employer in trouble.
