-- Issue #81: recording a Leaver's end date is a master-data act with its own
-- ActionLog entry, the same pattern `person_particulars_corrected` and
-- `person_full_name_corrected` already follow.
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
        'standing_proposals_refreshed',
        'employment_end_date_recorded'
    ));
