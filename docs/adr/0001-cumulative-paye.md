# Cumulative PAYE against year-to-date, not annualised months

Namibian PAYE uses annual progressive bands but is withheld monthly, so a monthly figure has to be derived. Salt calculates PAYE cumulatively: tax owed on year-to-date taxable remuneration, less PAYE already withheld this tax year. We rejected annualising the current period (`month × 12 ÷ 12`), which most local payroll vendors use.

Cumulative costs more up front — every calculation needs a `YearToDateContext`, and employers joining mid-year must supply opening balances — but it handles irregular remuneration correctly without special cases, and it is self-correcting: a fixed earlier period is absorbed by the next calculation rather than cascading through every later one. That self-correction is what makes ADR-0002 cheap.

## Consequences

- `YearToDateContext` (prior taxable remuneration, prior PAYE, periods elapsed) is a required part of every `PayrollInput`.
- An `OpeningBalance` per Employment per TaxYear is required for any employer starting mid-year.
- Year-to-date is always summed from live, non-reversed `FinalizedPayroll` records plus the `OpeningBalance`. It is never stored as a running total.
