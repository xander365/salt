-- OpeningBalance (§4.5): an affirmative, never auto-created fact. The four
-- boundary guards and the freeze-on-first-finalization rule are domain
-- reasoning Rust enforces (§11); the database only holds one row per
-- Employment per TaxYear.
CREATE TABLE opening_balance (
    id                         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    employment_id              TEXT NOT NULL REFERENCES employment (id),
    tax_year                   INTEGER NOT NULL,
    first_salt_period_end      DATE NOT NULL,
    prior_taxable_remuneration NUMERIC NOT NULL,
    prior_paye                 NUMERIC NOT NULL,
    reason                     TEXT,
    created_at                 TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by                 TEXT NOT NULL,
    UNIQUE (employment_id, tax_year)
);
