-- Issue #80 (parent #70): a run may override or remove a proposed standing
-- line for that run only, with a reason, leaving the standing record
-- untouched. `payroll_run_pay_line` already carries every column this needs
-- (migrations 0036, 0038) and its CHECKs already enforce the shapes in SQL;
-- this migration adds the one new column history needs.
--
-- `pay_line_provenance_json` freezes, at finalization, each line's source and
-- override/removal reason (ADR-0004: an old payroll's workings must never be
-- rebuilt from today's standing records). Nullable and never backfilled: a
-- row finalized before this shipped never had a provenance snapshot to
-- freeze, and there are no in-place JSON migrations, ever (§9.1's own rule).
ALTER TABLE finalized_payroll ADD COLUMN pay_line_provenance_json JSONB;

-- A new set of acts for the audit trail (§10): overriding and removing a
-- standing line for one run, and refreshing a run's proposals.
ALTER TABLE action_log_entry DROP CONSTRAINT action_log_entry_action_type_check;
ALTER TABLE action_log_entry ADD CONSTRAINT action_log_entry_action_type_check
    CHECK (action_type IN (
        'payroll_run_created',
        'employment_removed_from_run',
        'employment_added_to_correction_run',
        'payroll_finalized',
        'finalized_payroll_reversed',
        'opening_balance_created',
        'opening_balance_changed',
        'prior_employment_declared',
        'prior_employment_changed',
        'compensation_terms_corrected',
        'unsupported_deduction_status_corrected',
        'pay_schedule_changed',
        'employment_voided',
        'employer_particulars_corrected',
        'person_particulars_corrected',
        'person_full_name_corrected',
        'standing_pay_item_created',
        'standing_pay_item_ended',
        'standing_pay_line_overridden',
        'standing_pay_line_removed',
        'standing_proposals_refreshed'
    ));

-- Restate the restricted role's permission matrix, as every migration since
-- 0017 does. `pay_line_provenance_json` is written only by the finalization
-- INSERT, which the existing ALL-TABLES grant below already covers; no new
-- grant is needed, and the existing REVOKE UPDATE, DELETE on finalized_payroll
-- already refuses rewriting it in place.
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO payroll_app;
REVOKE UPDATE, DELETE ON finalized_payroll, reversal, action_log_entry, person, standing_pay_item FROM payroll_app;
REVOKE DELETE ON operator, employer_membership FROM payroll_app;
GRANT UPDATE (full_name) ON person TO payroll_app;
GRANT UPDATE (ended_at, ended_by, ended_reason) ON standing_pay_item TO payroll_app;
