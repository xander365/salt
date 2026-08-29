-- PayrollRunEmployment (§4.8). Membership semantics differ by run kind and
-- are opposites: an Ordinary run auto-proposes every overlapping Employment
-- and removal is the deliberate act (removed_at/by/reason, set together);
-- a Correction run proposes nothing and holds exactly one Employment.
--
-- replaces_finalized_payroll_id here is the *declared target* of a run that
-- has not happened yet — working state, not the recorded lineage. It is
-- deliberately NOT unique on this table (§4.8): two draft Correction runs
-- may both name the same target, and the first to finalize wins, the same
-- shape as the liveness primary key. The UNIQUE constraint lives on
-- finalized_payroll instead.
CREATE TABLE payroll_run_employment (
    payroll_run_id                 UUID NOT NULL REFERENCES payroll_run (id),
    employment_id                  TEXT NOT NULL REFERENCES employment (id),
    removed_at                     TIMESTAMPTZ,
    removed_by                     TEXT,
    removal_reason                 TEXT,
    replaces_finalized_payroll_id  UUID REFERENCES finalized_payroll (id),
    PRIMARY KEY (payroll_run_id, employment_id),
    CHECK (
        (removed_at IS NULL AND removed_by IS NULL AND removal_reason IS NULL)
        OR (
            removed_at IS NOT NULL AND removed_by IS NOT NULL
            AND removal_reason IS NOT NULL AND removal_reason <> ''
        )
    )
);

-- "At most one membership row when kind = Correction" (§11) spans two
-- tables, so a plain CHECK cannot express it — a trigger is the structural,
-- concurrency-safe equivalent for a rule a single-table CHECK cannot state.
CREATE FUNCTION enforce_correction_run_single_membership() RETURNS TRIGGER AS $$
DECLARE
    run_kind TEXT;
    member_count INTEGER;
BEGIN
    SELECT kind INTO run_kind FROM payroll_run WHERE id = NEW.payroll_run_id;
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

CREATE TRIGGER payroll_run_employment_correction_cardinality
    AFTER INSERT ON payroll_run_employment
    FOR EACH ROW
    EXECUTE FUNCTION enforce_correction_run_single_membership();
