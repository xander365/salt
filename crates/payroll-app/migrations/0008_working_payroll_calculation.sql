-- WorkingPayrollCalculation (§4.9): working state, overwritten on
-- recalculation. One row per (run, employment) is the natural primary key.
CREATE TABLE working_payroll_calculation (
    payroll_run_id           UUID NOT NULL REFERENCES payroll_run (id),
    employment_id             TEXT NOT NULL REFERENCES employment (id),
    payroll_input_json        JSONB NOT NULL,
    payroll_rules_json        JSONB NOT NULL,
    payroll_calculation_json  JSONB NOT NULL,
    calculated_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    calculated_by             TEXT NOT NULL,
    PRIMARY KEY (payroll_run_id, employment_id)
);
