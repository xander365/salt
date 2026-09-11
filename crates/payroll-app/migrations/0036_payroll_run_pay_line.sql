-- Issue #77 (parent #70): generalizes the run-scoped Earning table into a
-- PayLine table that can say where each line came from. `source` is
-- 'one_off' (typed directly on this run, never recurs), 'from_reversed_snapshot'
-- (copied from a reversed predecessor's frozen snapshot at correction
-- pre-population) or 'standing' (proposed from a StandingPayItem). This
-- ticket writes only the first two. `standing_pay_item_id`, `override_reason`,
-- `removed` and `removed_reason` are added now, unused, so issue #79 needs no
-- second migration on this table.
CREATE TABLE payroll_run_pay_line (
    payroll_run_id UUID NOT NULL REFERENCES payroll_run (id),
    employment_id  TEXT NOT NULL REFERENCES employment (id),
    line           SMALLINT NOT NULL,
    pay_line_json  JSONB NOT NULL,
    source         TEXT NOT NULL
                       CHECK (source IN ('one_off', 'from_reversed_snapshot', 'standing')),
    standing_pay_item_id TEXT,
    override_reason TEXT,
    removed        BOOLEAN NOT NULL DEFAULT FALSE,
    removed_reason TEXT,
    PRIMARY KEY (payroll_run_id, employment_id, line),
    -- Mirrors migration 0017's `payroll_run_earning_belongs_to_a_run_member`:
    -- a run-scoped row belongs to a member of that run, membership being the
    -- one place inclusion is decided (§4.8).
    CONSTRAINT payroll_run_pay_line_belongs_to_a_run_member
        FOREIGN KEY (payroll_run_id, employment_id)
        REFERENCES payroll_run_employment (payroll_run_id, employment_id),
    -- The same "a reason is never only whitespace" rule migration 0023 gives
    -- every other reason column.
    CONSTRAINT payroll_run_pay_line_override_reason_states_something
        CHECK (override_reason IS NULL OR btrim(override_reason) <> ''),
    CONSTRAINT payroll_run_pay_line_removed_reason_states_something
        CHECK (removed_reason IS NULL OR btrim(removed_reason) <> '')
);

-- Every existing run Earning was typed directly on its run.
INSERT INTO payroll_run_pay_line
    (payroll_run_id, employment_id, line, pay_line_json, source)
SELECT payroll_run_id, employment_id, line, earning_json, 'one_off'
FROM payroll_run_earning;

DROP TABLE payroll_run_earning;

-- Restate the restricted role's permission matrix, as every migration since
-- 0017 does: `GRANT ... ON ALL TABLES` only covers the tables that exist at
-- the time it runs.
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO payroll_app;
REVOKE UPDATE, DELETE ON finalized_payroll, reversal, action_log_entry, person FROM payroll_app;
REVOKE DELETE ON operator, employer_membership FROM payroll_app;
GRANT UPDATE (full_name) ON person TO payroll_app;
