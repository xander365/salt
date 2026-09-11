-- Issue #77 (parent #70 §D-6): the provenance rules for payroll_run_pay_line,
-- in SQL rather than in discipline, as the house style has been since
-- migration 0016. Migration 0036 added the standing columns so issue #79
-- needs no second migration on this table; these constraints finish that
-- promise, so #79 only has to start writing 'standing' rows.
--
-- No existing row can violate any of them: 0036 migrated every earlier row
-- as 'one_off' with every standing column NULL or FALSE, and nothing has
-- written a standing column since.

-- A StandingPayItem id is a UUID (§D-5), like every id payroll-app mints.
-- The foreign key waits for the standing_pay_item table itself (issue #79).
ALTER TABLE payroll_run_pay_line
    ALTER COLUMN standing_pay_item_id TYPE UUID USING standing_pay_item_id::uuid;

ALTER TABLE payroll_run_pay_line
    -- A line names a StandingPayItem exactly when it was proposed from one.
    ADD CONSTRAINT payroll_run_pay_line_standing_item_iff_standing_source
        CHECK ((standing_pay_item_id IS NOT NULL) = (source = 'standing')),
    -- An override states how a line differs from its standing item, so only
    -- a standing line can carry one. A one-off line is simply edited.
    ADD CONSTRAINT payroll_run_pay_line_override_only_on_a_standing_line
        CHECK (override_reason IS NULL OR source = 'standing'),
    -- "Removed for this run only" is a statement about a standing line, and
    -- it always says why. A one-off line is deleted, not removed.
    ADD CONSTRAINT payroll_run_pay_line_removal_is_a_reasoned_standing_act
        CHECK (NOT removed OR (source = 'standing' AND removed_reason IS NOT NULL)),
    -- A removal reason on a line that is not removed would read as a removal
    -- that never happened.
    ADD CONSTRAINT payroll_run_pay_line_removed_reason_only_when_removed
        CHECK (removed OR removed_reason IS NULL);

-- Structural idempotence for proposal and refresh (§D-6): a retry, a
-- double-click or a re-run cannot insert a second line for the same
-- StandingPayItem. Lines with no standing item — one-off and
-- from-reversed-snapshot — are outside the index.
CREATE UNIQUE INDEX payroll_run_pay_line_one_line_per_standing_item
    ON payroll_run_pay_line (payroll_run_id, employment_id, standing_pay_item_id)
    WHERE standing_pay_item_id IS NOT NULL;

-- Restate the restricted role's permission matrix, as every migration since
-- 0017 does.
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO payroll_app;
REVOKE UPDATE, DELETE ON finalized_payroll, reversal, action_log_entry, person FROM payroll_app;
REVOKE DELETE ON operator, employer_membership FROM payroll_app;
GRANT UPDATE (full_name) ON person TO payroll_app;
