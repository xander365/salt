-- Issue #73 (parent #70 D-7; ADR-0004 applied, not amended — D28): finalizing
-- now freezes the EmployerParticulars, PersonParticulars and
-- PayslipTemplateVersion that produced each FinalizedPayroll, beside the
-- PayrollInput, PayrollRules and PayrollCalculation §9 already freezes.
--
-- All three columns are nullable, and stay nullable forever: existing rows
-- carry snapshot_schema_version 1 and are never backfilled, because there is
-- nothing true to backfill them with (an Employer or Person's *current*
-- particulars are not what was on the payslip a v1 row already issued).
-- `finalize_payroll_run` writes all three on every new row from
-- snapshot_schema_version 2 onward, but even a 2 can carry a null
-- employer_particulars_json or person_particulars_json's optional fields
-- absent — an Employer or Person that had recorded nothing yet. A reader
-- branches on whether a column is null, never on the version integer: that
-- is the one condition that stays honest across a later version 3 adding
-- further optional fields inside the same version-2 shape (Deep
-- Instructions, issue #73).
ALTER TABLE finalized_payroll
    ADD COLUMN employer_particulars_json JSONB,
    ADD COLUMN person_particulars_json   JSONB,
    ADD COLUMN payslip_template_version  TEXT;

-- No grant to restate: these are new columns on an existing table, not a
-- new table, and `finalized_payroll` already carries the table-level
-- REVOKE UPDATE, DELETE migration 0015 put on it — a column added to an
-- already-restricted table inherits the same restriction with nothing
-- further to grant or revoke.
