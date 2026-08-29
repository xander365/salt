-- Forward-only corrections that bring the schema back onto the design record
-- (docs/domain/payroll-run-persistence.md) and close the structural gaps
-- INV-104 names: no concurrency-safe structural invariant is left to
-- application memory.

-- §4.4 is explicit: CompensationTerms carries `effective_from` **only**. A row
-- is in force until the next row's `effective_from`, and INV-014 already pins
-- every `effective_from` to a pay-period start date. An `effective_until`
-- column is a second, independent statement of the same fact, so the two can
-- disagree — a gap or an overlap between rows becomes representable, and §6.5's
-- repair (split the row so March carries the true amount and the old row starts
-- in April) would have to update two columns to stay consistent.
ALTER TABLE compensation_terms DROP COLUMN effective_until;

-- A snapshot schema version is an integer starting at 1 (§9). Version 0 or a
-- negative version identifies no reader.
ALTER TABLE finalized_payroll
    ADD CONSTRAINT finalized_payroll_snapshot_schema_version_starts_at_one
    CHECK (snapshot_schema_version >= 1);

-- The permanent history row carries its own PayPeriod, so it needs the same
-- ordering guard the run row already has.
ALTER TABLE finalized_payroll
    ADD CONSTRAINT finalized_payroll_period_is_ordered
    CHECK (period_end >= period_start);

-- A FinalizedPayroll names its Employment and its Employer independently, so
-- without a constraint the pair can disagree — and the disagreement would be
-- permanent, because no role may correct this table.
ALTER TABLE employment
    ADD CONSTRAINT employment_id_employer_unique UNIQUE (id, employer_id);

ALTER TABLE finalized_payroll
    ADD CONSTRAINT finalized_payroll_employment_belongs_to_its_employer
    FOREIGN KEY (employment_id, employer_id) REFERENCES employment (id, employer_id);

-- Payroll order is PayPeriod end date, never finalization timestamp (§7). That
-- only holds if the frozen period agrees with the run it came from, and the
-- employer with it.
ALTER TABLE payroll_run
    ADD CONSTRAINT payroll_run_id_employer_period_unique
    UNIQUE (id, employer_id, period_start, period_end);

ALTER TABLE finalized_payroll
    ADD CONSTRAINT finalized_payroll_matches_its_run
    FOREIGN KEY (payroll_run_id, employer_id, period_start, period_end)
    REFERENCES payroll_run (id, employer_id, period_start, period_end);

-- Run-scoped rows belong to members of that run. Membership is the one place
-- inclusion is decided — auto-proposed and reasonably removed for an Ordinary
-- run, explicit and singular for a Correction run (§4.8) — so an Earning line,
-- a WorkingCalculation, or a FinalizedPayroll for a non-member is a row that
-- bypassed that decision. A removed member keeps its membership row, so a
-- reasoned removal is unaffected.
ALTER TABLE payroll_run_earning
    ADD CONSTRAINT payroll_run_earning_belongs_to_a_run_member
    FOREIGN KEY (payroll_run_id, employment_id)
    REFERENCES payroll_run_employment (payroll_run_id, employment_id);

ALTER TABLE working_payroll_calculation
    ADD CONSTRAINT working_payroll_calculation_belongs_to_a_run_member
    FOREIGN KEY (payroll_run_id, employment_id)
    REFERENCES payroll_run_employment (payroll_run_id, employment_id);

ALTER TABLE finalized_payroll
    ADD CONSTRAINT finalized_payroll_belongs_to_a_run_member
    FOREIGN KEY (payroll_run_id, employment_id)
    REFERENCES payroll_run_employment (payroll_run_id, employment_id);

-- The year-to-date read (§8) is: join liveness, then sum this Employment's
-- TaxYear rows whose period_end precedes this one. The liveness primary key
-- serves its half; this index serves the other.
CREATE INDEX finalized_payroll_year_to_date
    ON finalized_payroll (employment_id, tax_year, period_end);

-- Reversal deletes the liveness row by the FinalizedPayroll it names, and the
-- liveness primary key does not lead with that column.
CREATE INDEX live_finalized_payroll_by_finalized_payroll
    ON live_finalized_payroll (finalized_payroll_id);

-- Both trigger functions run unqualified table names under the caller's
-- search_path. Pinning it here means the guard reads payroll_run and
-- payroll_run_employment, and never a same-named relation earlier on some
-- caller's path.
CREATE OR REPLACE FUNCTION enforce_correction_run_single_membership() RETURNS TRIGGER
    LANGUAGE plpgsql
    SET search_path = pg_catalog, public
AS $$
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
$$;

CREATE OR REPLACE FUNCTION enforce_correction_run_kind_cardinality() RETURNS TRIGGER
    LANGUAGE plpgsql
    SET search_path = pg_catalog, public
AS $$
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
$$;

-- Re-applying the permission set. #[sqlx::test] builds a fresh database per
-- test, so 0015's `GRANT ... ON ALL TABLES` covered exactly the tables that
-- existed when it ran; a table added by a migration after it would silently
-- reach the application role with no grant at all. Restating the set here
-- keeps the two migrations' intent identical, and a test asserts the whole
-- grant matrix so the next table cannot be forgotten quietly.
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO payroll_app;
REVOKE UPDATE, DELETE ON finalized_payroll, reversal, action_log_entry FROM payroll_app;
