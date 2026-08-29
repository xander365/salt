-- PayrollRunEarning (§4.5d): run-scoped. Absence means no additional
-- earnings, and that is a complete statement — no ConfirmedNone ceremony.
CREATE TABLE payroll_run_earning (
    payroll_run_id UUID NOT NULL REFERENCES payroll_run (id),
    employment_id  TEXT NOT NULL REFERENCES employment (id),
    line           SMALLINT NOT NULL,
    earning_json   JSONB NOT NULL,
    PRIMARY KEY (payroll_run_id, employment_id, line)
);
