-- Correction lineage, moved from application memory into PostgreSQL as far
-- as it goes (§4.8, §11, INV-104). §4.8 names four things the database
-- enforces about lineage — "UNIQUE, the foreign key, the matching employment
-- and period, and correction_reason non-empty when kind = Correction". The
-- first, second and fourth shipped with the schema; the third did not, and
-- only Rust's `validate_correction_target` was holding it.
--
-- It matters more here than the same claim would elsewhere, because
-- `finalized_payroll` is the one table no role may correct: a replacement
-- pointing at another Employment's record, or at the same Employment's
-- record for a different PayPeriod, would be a permanent lie about what was
-- replaced, and §13's whole success condition is that an auditor reads the
-- story off the rows.

-- The recorded lineage (§9): a replacement and its target are one
-- Employment's payroll for one PayPeriod. `UNIQUE (id, employment_id,
-- period_end)` already exists for the liveness foreign key (migration 0016),
-- so this reuses it rather than adding a second index saying the same thing.
--
-- MATCH SIMPLE is what makes the two null-lineage cases §4.8 allows still
-- pass: with replaces_finalized_payroll_id NULL the constraint is not
-- checked at all, and which of the two cases applies stays Rust's question.
ALTER TABLE finalized_payroll
    ADD CONSTRAINT finalized_payroll_replacement_matches_its_target
    FOREIGN KEY (replaces_finalized_payroll_id, employment_id, period_end)
    REFERENCES finalized_payroll (id, employment_id, period_end);

-- The declared target (§4.8's other table): working state, so the period it
-- must match lives on payroll_run rather than on the membership row, and the
-- database can only hold half of the pair here. It holds that half — a
-- Correction run declaring another Employment's FinalizedPayroll as its
-- target is refused outright, rather than at whichever later moment someone
-- reads it.
ALTER TABLE finalized_payroll
    ADD CONSTRAINT finalized_payroll_id_employment_unique UNIQUE (id, employment_id);

ALTER TABLE payroll_run_employment
    ADD CONSTRAINT payroll_run_employment_target_matches_its_employment
    FOREIGN KEY (replaces_finalized_payroll_id, employment_id)
    REFERENCES finalized_payroll (id, employment_id);

-- "replaces_finalized_payroll_id is Correction only" (§4.8) spans two tables
-- the same way the single-membership rule does, so it needs the same shape of
-- guard rather than a CHECK.
--
-- Finalization copies each member's declared target into its FinalizedPayroll
-- unconditionally, by kind — an Ordinary member's is always NULL, so nothing
-- decides which branch to take from. A membership row that carried one
-- anyway, or a Correction run reclassified as Ordinary while its member still
-- named a target, would put recorded lineage on a row that replaced nothing,
-- and no role could then correct it.
CREATE FUNCTION enforce_declared_target_is_correction_only() RETURNS TRIGGER
    LANGUAGE plpgsql
    SET search_path = pg_catalog, public
AS $$
DECLARE
    run_kind TEXT;
BEGIN
    IF NEW.replaces_finalized_payroll_id IS NULL THEN
        RETURN NEW;
    END IF;

    SELECT kind INTO run_kind FROM payroll_run WHERE id = NEW.payroll_run_id;

    IF run_kind <> 'correction' THEN
        RAISE EXCEPTION
            'only a Correction run declares a replacement target (payroll_run_id %)',
            NEW.payroll_run_id;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER payroll_run_employment_target_is_correction_only
    AFTER INSERT OR UPDATE OF replaces_finalized_payroll_id, payroll_run_id
    ON payroll_run_employment
    FOR EACH ROW
    EXECUTE FUNCTION enforce_declared_target_is_correction_only();

-- The other direction: the run changes kind under a membership row that
-- already declares a target. This is the lineage half of what migration
-- 0016's `payroll_run_correction_cardinality_on_kind_change` does for
-- membership count.
CREATE FUNCTION enforce_ordinary_run_declares_no_target() RETURNS TRIGGER
    LANGUAGE plpgsql
    SET search_path = pg_catalog, public
AS $$
DECLARE
    declaring_member TEXT;
BEGIN
    IF NEW.kind = 'correction' THEN
        RETURN NEW;
    END IF;

    SELECT employment_id INTO declaring_member
    FROM payroll_run_employment
    WHERE payroll_run_id = NEW.id AND replaces_finalized_payroll_id IS NOT NULL
    LIMIT 1;

    IF declaring_member IS NOT NULL THEN
        RAISE EXCEPTION
            'only a Correction run declares a replacement target (payroll_run_id %, employment %)',
            NEW.id, declaring_member;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER payroll_run_declares_no_target_when_ordinary
    AFTER UPDATE OF kind ON payroll_run
    FOR EACH ROW
    EXECUTE FUNCTION enforce_ordinary_run_declares_no_target();
