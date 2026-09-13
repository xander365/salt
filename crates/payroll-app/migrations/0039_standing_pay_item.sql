-- StandingPayItem (issue #79, parent #70 §0): an Earning or Deduction an
-- Employment carries with an effective-from date, which every new Ordinary
-- run proposes on its own (migration 0036 already added the columns this
-- proposal writes into `payroll_run_pay_line`). `pay_line_json` holds the
-- same `PayLineInstruction` shape a pay line stores, restricted in Rust to
-- the two kinds that make sense standing (a taxable allowance, a medical
-- aid premium) — never `Overtime`, which is hours for one period, not a
-- recurring fact.
--
-- Ended by `ended_at`/`ended_by`/`ended_reason`, never a delete: a historical
-- proposal must always have something to point at, the same reasoning
-- migration 0012 gives `payroll_run_employment.removed_at` and migration
-- 0028 gives a disabled Operator.
CREATE TABLE standing_pay_item (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    employment_id  TEXT NOT NULL REFERENCES employment (id),
    pay_line_json  JSONB NOT NULL,
    effective_from DATE NOT NULL,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by     TEXT NOT NULL,
    ended_at       TIMESTAMPTZ,
    ended_by       TEXT,
    ended_reason   TEXT,
    CHECK (
        (ended_at IS NULL AND ended_by IS NULL AND ended_reason IS NULL)
        OR (
            ended_at IS NOT NULL AND ended_by IS NOT NULL
            AND ended_reason IS NOT NULL AND ended_reason <> ''
        )
    )
);

-- Every proposal at run creation reads "the items in force for this
-- Employment as of a date", so that is the lookup this index serves.
CREATE INDEX standing_pay_item_employment_effective_from
    ON standing_pay_item (employment_id, effective_from);

-- The foreign key migration 0038 left waiting for this table (issue #79):
-- a `payroll_run_pay_line.standing_pay_item_id` now names a real row.
ALTER TABLE payroll_run_pay_line
    ADD CONSTRAINT payroll_run_pay_line_standing_pay_item_id_fkey
        FOREIGN KEY (standing_pay_item_id) REFERENCES standing_pay_item (id);

-- Restate the restricted role's permission matrix, as every migration since
-- 0017 does.
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO payroll_app;
REVOKE UPDATE, DELETE ON finalized_payroll, reversal, action_log_entry, person FROM payroll_app;
REVOKE DELETE ON operator, employer_membership, standing_pay_item FROM payroll_app;
GRANT UPDATE (full_name) ON person TO payroll_app;

-- A new act for the audit trail (§10): recording and ending a StandingPayItem.
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
        'standing_pay_item_ended'
    ));
