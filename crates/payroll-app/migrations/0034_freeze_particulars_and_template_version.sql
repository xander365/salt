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

-- Restate the restricted role's permission matrix, as every migration since
-- 0017 does. The table-level revoke continues to cover columns added above;
-- repeating it here makes that invariant survive a future change to the
-- broad grant without relying on migration 0015 remaining the last word.
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO payroll_app;
REVOKE UPDATE, DELETE ON finalized_payroll, reversal, action_log_entry, person FROM payroll_app;
REVOKE DELETE ON operator, employer_membership FROM payroll_app;
GRANT UPDATE (full_name) ON person TO payroll_app;
