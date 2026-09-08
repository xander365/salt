-- PersonParticulars (issue #72, parent #70 D-7; CONTEXT.md's own glossary
-- entry): the Person's identity number and address, plus the ability to fix
-- a misspelled `full_name` after the fact. One `person_particulars` row per
-- Person, correctable forever with a stated reason — the same
-- `EmployerParticulars` pattern (migration 0032), Person-scoped instead of
-- Employer-scoped and never Owner-only (D25 restricts only Employer
-- particulars).
CREATE TABLE person_particulars (
    person_id       TEXT PRIMARY KEY REFERENCES person (id),
    identity_number TEXT NOT NULL,
    address_line1   TEXT NOT NULL,
    address_line2   TEXT,
    city            TEXT NOT NULL,
    postal_code     TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by      TEXT NOT NULL
);

ALTER TABLE person_particulars
    ADD CONSTRAINT person_particulars_identity_number_states_something
    CHECK (btrim(identity_number) <> '' AND identity_number ~ '[^[:space:]]');
ALTER TABLE person_particulars
    ADD CONSTRAINT person_particulars_address_line1_states_something
    CHECK (btrim(address_line1) <> '' AND address_line1 ~ '[^[:space:]]');
ALTER TABLE person_particulars
    ADD CONSTRAINT person_particulars_city_states_something
    CHECK (btrim(city) <> '' AND city ~ '[^[:space:]]');
ALTER TABLE person_particulars
    ADD CONSTRAINT person_particulars_created_by_names_an_actor
    CHECK (btrim(created_by) <> '');

ALTER TABLE person_particulars
    ADD CONSTRAINT person_particulars_address_line2_not_blank_if_present
    CHECK (address_line2 IS NULL OR btrim(address_line2) <> '');
ALTER TABLE person_particulars
    ADD CONSTRAINT person_particulars_postal_code_not_blank_if_present
    CHECK (postal_code IS NULL OR btrim(postal_code) <> '');

-- Two new ActionTypes, the same drop-and-readd every migration since 0014
-- gives `action_log_entry`'s own CHECK (PostgreSQL has no
-- `ALTER CONSTRAINT ... ADD VALUE`): one for a `person_particulars`
-- insert/correction, and one for a `full_name` correction — two different
-- facts about a Person, each with its own before/after shape, so each gets
-- its own ActionType rather than sharing one that would blur them together.
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
        'person_full_name_corrected'
    ));

-- `person` was made append-only by migration 0031 in anticipation of exactly
-- this ticket (that migration's own comment). A blanket UPDATE would let a
-- future use case rewrite `employer_id` or `created_by` — the two columns
-- that ADR-0020's scoping and §10's own attribution depend on — so this
-- grants `UPDATE` on `full_name` alone. Every other column on `person`
-- remains as unwritable to `payroll_app` as the day migration 0031 revoked
-- it.
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO payroll_app;
REVOKE UPDATE, DELETE ON finalized_payroll, reversal, action_log_entry, person FROM payroll_app;
REVOKE DELETE ON operator, employer_membership FROM payroll_app;
GRANT UPDATE (full_name) ON person TO payroll_app;
