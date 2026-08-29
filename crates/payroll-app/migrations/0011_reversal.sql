-- Reversal (§6.1): the original FinalizedPayroll is untouched. A reversal
-- cannot be undone, and there is at most one per finalized payroll.
CREATE TABLE reversal (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    finalized_payroll_id UUID NOT NULL REFERENCES finalized_payroll (id),
    reversed_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    reversed_by          TEXT NOT NULL,
    reason               TEXT NOT NULL CHECK (reason <> ''),
    UNIQUE (finalized_payroll_id)
);
