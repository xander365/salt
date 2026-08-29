-- `finalized_payroll` carries the same fact 0020 and 0021 already hardened
-- for `prior_employment_declaration` and `opening_balance`:
-- `taxable_remuneration` and `paye` are each a `Money` -- a whole,
-- non-negative number of cents -- and NUMERIC admits a fraction of a cent
-- and a negative that no reader's `Money::from_cents` can decode.
--
-- These two columns are the sharp edge of §8: year-to-date sums them, never
-- the JSON, so a fractional cent stored here would be silently rounded away
-- by the reader's cast and change a PAYE figure for the rest of the TaxYear.
-- BIGINT is what finalization will bind (`Money::cents()`) and what every
-- other Money column in the schema already is; the CHECK makes the reader's
-- reconstruction a schema guarantee rather than a hope about every writer.
--
-- Existing fractional values must make this migration stop: PostgreSQL
-- rounds NUMERIC-to-BIGINT casts, and silently changing a recorded figure
-- would manufacture a false payroll fact. An operator must remediate those
-- rows explicitly instead.

DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM finalized_payroll
        WHERE taxable_remuneration <> trunc(taxable_remuneration)
           OR paye <> trunc(paye)
    ) THEN
        RAISE EXCEPTION
            'finalized_payroll contains fractional-cent Money values; remediate them before migrating';
    END IF;
END
$$;

ALTER TABLE finalized_payroll
    ALTER COLUMN taxable_remuneration TYPE BIGINT USING taxable_remuneration::bigint,
    ALTER COLUMN paye TYPE BIGINT USING paye::bigint;

ALTER TABLE finalized_payroll
    ADD CONSTRAINT finalized_payroll_figures_are_money
    CHECK (taxable_remuneration >= 0 AND paye >= 0);
