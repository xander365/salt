-- `opening_balance` carries the same fact 0020 already hardened for
-- `prior_employment_declaration`: `prior_taxable_remuneration` and
-- `prior_paye` are each a `Money` -- a whole, non-negative number of cents --
-- and NUMERIC admits a fraction of a cent and a negative that no reader's
-- `Money::from_cents` can decode. BIGINT is what `record_opening_balance`
-- binds (`Money::cents()`) and what every other Money column in the schema
-- already is; the CHECK makes the reader's reconstruction a schema
-- guarantee rather than a hope about every writer.

ALTER TABLE opening_balance
    ALTER COLUMN prior_taxable_remuneration TYPE BIGINT USING prior_taxable_remuneration::bigint,
    ALTER COLUMN prior_paye TYPE BIGINT USING prior_paye::bigint;

ALTER TABLE opening_balance
    ADD CONSTRAINT opening_balance_figures_are_money
    CHECK (prior_taxable_remuneration >= 0 AND prior_paye >= 0);
