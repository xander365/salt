-- A missing WorkingCalculation can mean either that this member has not been
-- calculated yet, or that editing their pay lines deliberately retired older
-- figures. The run detail must distinguish those honest states after reload.
ALTER TABLE payroll_run_employment
    ADD COLUMN figures_invalidated_by_pay_line_write BOOLEAN NOT NULL DEFAULT FALSE;
