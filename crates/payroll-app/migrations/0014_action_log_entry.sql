-- ActionLogEntry (§10). action_type is a typed enum in Rust, never free
-- text; the CHECK is the database's own guard against a value Rust never
-- produces.
CREATE TABLE action_log_entry (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    employer_id TEXT NOT NULL REFERENCES employer (id),
    actor       TEXT NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    action_type TEXT NOT NULL CHECK (action_type IN (
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
        'employment_voided'
    )),
    target_type TEXT NOT NULL,
    target_id   TEXT NOT NULL,
    context     JSONB
);
