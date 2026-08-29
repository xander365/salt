-- Corrections to the initial schema. Migrations are forward-only: retain the
-- original trigger and replace its function with a version that serializes
-- membership changes on the PayrollRun row.
CREATE OR REPLACE FUNCTION enforce_correction_run_single_membership() RETURNS TRIGGER AS $$
DECLARE
    run_kind TEXT;
    member_count INTEGER;
BEGIN
    -- The parent-row lock makes concurrent membership writes for one run
    -- serial. It is held until the transaction ends, so the second writer
    -- counts the first writer's committed row before it may proceed.
    SELECT kind INTO run_kind
    FROM payroll_run
    WHERE id = NEW.payroll_run_id
    FOR UPDATE;

    IF run_kind = 'correction' THEN
        SELECT count(*) INTO member_count
        FROM payroll_run_employment
        WHERE payroll_run_id = NEW.payroll_run_id;

        IF member_count > 1 THEN
            RAISE EXCEPTION
                'a Correction run may hold only one Employment (payroll_run_id %)',
                NEW.payroll_run_id;
        END IF;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- Moving a membership row into a Correction run is another way to introduce a
-- second member, so it needs the same guard as insertion.
CREATE TRIGGER payroll_run_employment_correction_cardinality_on_move
    AFTER UPDATE OF payroll_run_id ON payroll_run_employment
    FOR EACH ROW
    EXECUTE FUNCTION enforce_correction_run_single_membership();

-- A run can otherwise be created as Ordinary, populated, then reclassified as
-- Correction. The row update itself serializes with the membership trigger's
-- FOR UPDATE lock, and this check rejects an invalid target state.
CREATE FUNCTION enforce_correction_run_kind_cardinality() RETURNS TRIGGER AS $$
DECLARE
    member_count INTEGER;
BEGIN
    IF NEW.kind = 'correction' THEN
        SELECT count(*) INTO member_count
        FROM payroll_run_employment
        WHERE payroll_run_id = NEW.id;

        IF member_count > 1 THEN
            RAISE EXCEPTION
                'a Correction run may hold only one Employment (payroll_run_id %)',
                NEW.id;
        END IF;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER payroll_run_correction_cardinality_on_kind_change
    AFTER UPDATE OF kind ON payroll_run
    FOR EACH ROW
    EXECUTE FUNCTION enforce_correction_run_kind_cardinality();

-- A liveness row must point at a FinalizedPayroll for its own Employment and
-- PayPeriod; otherwise a year-to-date join can read someone else's payroll.
ALTER TABLE finalized_payroll
    ADD CONSTRAINT finalized_payroll_id_employment_period_unique
    UNIQUE (id, employment_id, period_end);

ALTER TABLE live_finalized_payroll
    ADD CONSTRAINT live_finalized_payroll_matches_finalized_payroll
    FOREIGN KEY (finalized_payroll_id, employment_id, period_end)
    REFERENCES finalized_payroll (id, employment_id, period_end);

-- Reversals are immutable history and the action log is append-only, so the
-- restricted application role may create them but cannot revise or erase them.
REVOKE UPDATE, DELETE ON reversal, action_log_entry FROM payroll_app;
