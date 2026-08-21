# Corrections are reversal plus replacement, never edits

A `FinalizedPayroll` is immutable (INV-004). When an employer finds an error in a finalized period, Salt records an explicit `Reversal` cancelling that record, and optionally a fresh replacement calculation. History then holds all three: original, reversal, replacement. We rejected editing finalized records, and rejected delta-only adjustments posted into the next period.

A replacement is optional — a bare `Reversal` is valid when someone was paid who should not have been. Corrections do not cascade: later finalized periods are left alone, because cumulative PAYE (ADR-0001) absorbs the difference at the next calculation. Cascading would let one fix rewrite a year of history and re-issue months of payslips.

## Consequences

- Reversal and replacement are per Employment, not per `PayrollRun`.
- Because year-to-date sums only live records, a reversal is arithmetically complete on its own.
