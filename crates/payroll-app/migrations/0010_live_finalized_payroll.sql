-- Liveness is a table, not a column (§6.2). Finalizing inserts; reversal
-- deletes; a replacement inserts again. The primary key is the whole
-- concurrency guard: a second live row for the same (employment, period_end)
-- is unrepresentable, whatever two application processes believe (§5.4).
CREATE TABLE live_finalized_payroll (
    employment_id        TEXT NOT NULL REFERENCES employment (id),
    period_end           DATE NOT NULL,
    finalized_payroll_id UUID NOT NULL REFERENCES finalized_payroll (id),
    PRIMARY KEY (employment_id, period_end)
);
