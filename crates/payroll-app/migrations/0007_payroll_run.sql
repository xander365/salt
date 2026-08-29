-- PayrollRun (§4.6, §4.7). status is a stored column, not derived: the
-- finalization transaction takes SELECT ... FOR UPDATE on this row, and a
-- derived status offers nothing to lock (§4.6).
CREATE TABLE payroll_run (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    employer_id       TEXT NOT NULL REFERENCES employer (id),
    period_start      DATE NOT NULL,
    period_end        DATE NOT NULL,
    pay_date          DATE NOT NULL,
    kind              TEXT NOT NULL CHECK (kind IN ('ordinary', 'correction')),
    status            TEXT NOT NULL CHECK (status IN ('draft', 'calculated', 'finalized')),
    correction_reason TEXT,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by        TEXT NOT NULL,
    CHECK (period_end >= period_start),
    CHECK (
        (kind = 'correction' AND correction_reason IS NOT NULL AND correction_reason <> '')
        OR (kind = 'ordinary' AND correction_reason IS NULL)
    )
);

-- One Ordinary run per Employer + PayPeriod (§4.6); Correction runs are
-- unconstrained in number.
CREATE UNIQUE INDEX one_ordinary_payroll_run_per_employer_and_period
    ON payroll_run (employer_id, period_end)
    WHERE kind = 'ordinary';
