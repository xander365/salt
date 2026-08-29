-- Migration 0019 made every attribution and every mandatory reason
-- non-empty. `<> ''` refuses the empty string and nothing else, so a single
-- space still passes: `removal_reason = ' '` satisfies the CHECK while
-- answering "why was this person not paid" with nothing, and `created_by =
-- ' '` answers "who did this" with nothing. Both are unfixable after the
-- fact, because the application role may not UPDATE the ActionLog and may
-- not UPDATE a finalized payroll at all (migration 0015).
--
-- `btrim` closes that: a value made only of whitespace is refused exactly
-- like the empty string it means. The constraints are added beside 0019's
-- rather than replacing them, so each stays a separate, individually named
-- statement about the column and the older one keeps its own error text.
--
-- The sweep covers every column of both kinds, not only the PayrollRun ones
-- issue #29 introduces, because a rule that holds on some attribution
-- columns and not others is a rule nobody can rely on.

-- Reasons. Each of these is the thing the Employer needs six months later
-- (§4.8, §6.2); whitespace states nothing.
ALTER TABLE payroll_run
    ADD CONSTRAINT payroll_run_correction_reason_states_something
    CHECK (correction_reason IS NULL OR btrim(correction_reason) <> '');
ALTER TABLE payroll_run_employment
    ADD CONSTRAINT payroll_run_employment_removal_reason_states_something
    CHECK (removal_reason IS NULL OR btrim(removal_reason) <> '');
ALTER TABLE reversal
    ADD CONSTRAINT reversal_reason_states_something
    CHECK (btrim(reason) <> '');

-- Attribution.
ALTER TABLE employer
    ADD CONSTRAINT employer_created_by_names_an_actor CHECK (btrim(created_by) <> '');
ALTER TABLE employment
    ADD CONSTRAINT employment_created_by_names_an_actor CHECK (btrim(created_by) <> '');
ALTER TABLE compensation_terms
    ADD CONSTRAINT compensation_terms_created_by_names_an_actor CHECK (btrim(created_by) <> '');
ALTER TABLE opening_balance
    ADD CONSTRAINT opening_balance_created_by_names_an_actor CHECK (btrim(created_by) <> '');
ALTER TABLE prior_employment_declaration
    ADD CONSTRAINT prior_employment_declaration_declared_by_names_an_actor
    CHECK (btrim(declared_by) <> '');
ALTER TABLE unsupported_deduction_declaration
    ADD CONSTRAINT unsupported_deduction_declaration_declared_by_names_an_actor
    CHECK (btrim(declared_by) <> '');
ALTER TABLE payroll_run
    ADD CONSTRAINT payroll_run_created_by_names_an_actor CHECK (btrim(created_by) <> '');
ALTER TABLE payroll_run_employment
    ADD CONSTRAINT payroll_run_employment_removed_by_names_an_actor
    CHECK (removed_by IS NULL OR btrim(removed_by) <> '');
ALTER TABLE working_payroll_calculation
    ADD CONSTRAINT working_payroll_calculation_calculated_by_names_an_actor
    CHECK (btrim(calculated_by) <> '');
ALTER TABLE finalized_payroll
    ADD CONSTRAINT finalized_payroll_finalized_by_names_an_actor
    CHECK (btrim(finalized_by) <> '');
ALTER TABLE reversal
    ADD CONSTRAINT reversal_reversed_by_names_an_actor CHECK (btrim(reversed_by) <> '');

-- The log's own row: who acted, and what the entry is about.
ALTER TABLE action_log_entry
    ADD CONSTRAINT action_log_entry_actor_names_an_actor CHECK (btrim(actor) <> '');
ALTER TABLE action_log_entry
    ADD CONSTRAINT action_log_entry_target_states_something
    CHECK (btrim(target_type) <> '' AND btrim(target_id) <> '');

-- The Employment's Person, for the same reason 0019 gave: a row naming its
-- Person with whitespace names nobody.
ALTER TABLE employment
    ADD CONSTRAINT employment_names_a_person_not_whitespace CHECK (btrim(person_id) <> '');
