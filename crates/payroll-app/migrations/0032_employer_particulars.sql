-- EmployerParticulars (issue #71, parent #70 D-7): the Employer's registered
-- name, address and statutory registration numbers, as CONTEXT.md's own
-- glossary entry describes them. One row per Employer, correctable forever
-- with a stated reason — the same `CompensationTerms` correction pattern
-- (§6.5), generalized to a singleton fact that carries no `effective_from`
-- of its own to split "insert" from "correct" by. Nothing here freezes it
-- into a FinalizedPayroll: that is a later ticket (#70 D-7's own scope
-- boundary), so this table is read live by whatever renders a payslip until
-- that ticket lands.
CREATE TABLE employer_particulars (
    employer_id            TEXT PRIMARY KEY REFERENCES employer (id),
    registered_name        TEXT NOT NULL,
    address_line1          TEXT NOT NULL,
    address_line2          TEXT,
    city                   TEXT NOT NULL,
    postal_code            TEXT,
    income_tax_number      TEXT,
    social_security_number TEXT,
    created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by             TEXT NOT NULL
);

-- The same blank-and-whitespace guard 0019/0023 put on every mandatory
-- column, applied here at creation rather than added on later: this table
-- has no earlier migration to have missed it.
ALTER TABLE employer_particulars
    ADD CONSTRAINT employer_particulars_registered_name_states_something
    CHECK (btrim(registered_name) <> '' AND registered_name ~ '[^[:space:]]');
ALTER TABLE employer_particulars
    ADD CONSTRAINT employer_particulars_address_line1_states_something
    CHECK (btrim(address_line1) <> '' AND address_line1 ~ '[^[:space:]]');
ALTER TABLE employer_particulars
    ADD CONSTRAINT employer_particulars_city_states_something
    CHECK (btrim(city) <> '' AND city ~ '[^[:space:]]');
ALTER TABLE employer_particulars
    ADD CONSTRAINT employer_particulars_created_by_names_an_actor
    CHECK (btrim(created_by) <> '');

-- The four optional columns: unset is `NULL`, but a value made only of
-- whitespace states nothing and is refused exactly like the mandatory ones.
ALTER TABLE employer_particulars
    ADD CONSTRAINT employer_particulars_address_line2_not_blank_if_present
    CHECK (address_line2 IS NULL OR btrim(address_line2) <> '');
ALTER TABLE employer_particulars
    ADD CONSTRAINT employer_particulars_postal_code_not_blank_if_present
    CHECK (postal_code IS NULL OR btrim(postal_code) <> '');
ALTER TABLE employer_particulars
    ADD CONSTRAINT employer_particulars_income_tax_number_not_blank_if_present
    CHECK (income_tax_number IS NULL OR btrim(income_tax_number) <> '');
ALTER TABLE employer_particulars
    ADD CONSTRAINT employer_particulars_social_security_number_not_blank_if_present
    CHECK (social_security_number IS NULL OR btrim(social_security_number) <> '');

-- A new `ActionType` reaches the ActionLog's own CHECK the same way every
-- earlier one has: dropped and re-added with the fuller list, because
-- PostgreSQL has no `ALTER CONSTRAINT ... ADD VALUE`.
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
        'employer_particulars_corrected'
    ));

-- Restating the grant set, as every migration since 0017 does after a
-- schema change: `GRANT ... ON ALL TABLES` only covers the tables that
-- exist when it runs, so restating keeps the whole matrix, `employer_particulars`
-- included, provably intended rather than merely inherited.
-- `employer_particulars` is correctable master data, like `compensation_terms`,
-- so it keeps the full grant — there is no UPDATE to revoke from it.
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO payroll_app;
REVOKE UPDATE, DELETE ON finalized_payroll, reversal, action_log_entry, person FROM payroll_app;
REVOKE DELETE ON operator, employer_membership FROM payroll_app;
