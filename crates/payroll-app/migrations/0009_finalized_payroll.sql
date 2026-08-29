-- FinalizedPayroll (§9): the immutable history row. taxable_remuneration
-- and paye are real numeric columns beside the frozen JSONB snapshots
-- because year-to-date reads them, never the JSON (§8, §9, ADR-0012).
--
-- replaces_finalized_payroll_id is UNIQUE here, and only here (§4.8) — a
-- reversed FinalizedPayroll can be replaced at most once, which is what
-- turns repeated corrections into a chain rather than a tree. It is
-- nullable for the two cases §4.8 names (a reasoned removal, or the
-- Employment never having been a member); Rust decides which applies.
--
-- UPDATE and DELETE are revoked from the application role in a later
-- migration, once this table exists.
CREATE TABLE finalized_payroll (
    id                            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    payroll_run_id                UUID NOT NULL REFERENCES payroll_run (id),
    employment_id                 TEXT NOT NULL REFERENCES employment (id),
    employer_id                   TEXT NOT NULL REFERENCES employer (id),
    period_start                  DATE NOT NULL,
    period_end                    DATE NOT NULL,
    tax_year                      INTEGER NOT NULL,
    replaces_finalized_payroll_id UUID REFERENCES finalized_payroll (id),

    payroll_input_json            JSONB NOT NULL,
    payroll_rules_json            JSONB NOT NULL,
    payroll_calculation_json      JSONB NOT NULL,

    taxable_remuneration          NUMERIC NOT NULL,
    paye                          NUMERIC NOT NULL,

    paye_table_id                 TEXT NOT NULL,
    ssc_rules_id                  TEXT NOT NULL,
    salt_version                  TEXT NOT NULL,
    snapshot_schema_version       INTEGER NOT NULL DEFAULT 1,

    finalized_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    finalized_by                  TEXT NOT NULL,

    UNIQUE (replaces_finalized_payroll_id)
);
