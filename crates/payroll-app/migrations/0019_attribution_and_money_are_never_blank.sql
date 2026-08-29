-- Two facts the schema stated only by convention.
--
-- 1. `basic_pay` is a `Money` amount and `Money` is never negative, but the
--    column let any BIGINT in. `get_employment_snapshot` reconstructs a
--    `Money` from it and can only `expect` that reconstruction to succeed;
--    the CHECK is what makes that expectation a schema guarantee rather
--    than a hope about every past and future writer.
--
-- 2. §10 makes the ActionLog "who did what and when". An empty actor is a
--    row that answers "who" with nothing, and it is unfixable after the
--    fact because the application role may not UPDATE the log. Every
--    attribution column in the schema gets the same guard, so a blank
--    actor is refused at the one place all of them pass through, and
--    attribution never depends on a caller remembering to pass a name.

ALTER TABLE compensation_terms
    ADD CONSTRAINT compensation_terms_basic_pay_is_not_negative
    CHECK (basic_pay >= 0);

ALTER TABLE employment
    ADD CONSTRAINT employment_names_a_person
    CHECK (person_id <> '');

ALTER TABLE employer
    ADD CONSTRAINT employer_created_by_is_attributed CHECK (created_by <> '');
ALTER TABLE employment
    ADD CONSTRAINT employment_created_by_is_attributed CHECK (created_by <> '');
ALTER TABLE compensation_terms
    ADD CONSTRAINT compensation_terms_created_by_is_attributed CHECK (created_by <> '');
ALTER TABLE opening_balance
    ADD CONSTRAINT opening_balance_created_by_is_attributed CHECK (created_by <> '');
ALTER TABLE prior_employment_declaration
    ADD CONSTRAINT prior_employment_declaration_declared_by_is_attributed
    CHECK (declared_by <> '');
ALTER TABLE unsupported_deduction_declaration
    ADD CONSTRAINT unsupported_deduction_declaration_declared_by_is_attributed
    CHECK (declared_by <> '');
ALTER TABLE payroll_run
    ADD CONSTRAINT payroll_run_created_by_is_attributed CHECK (created_by <> '');
ALTER TABLE payroll_run_employment
    ADD CONSTRAINT payroll_run_employment_removed_by_is_attributed
    CHECK (removed_by IS NULL OR removed_by <> '');
ALTER TABLE working_payroll_calculation
    ADD CONSTRAINT working_payroll_calculation_calculated_by_is_attributed
    CHECK (calculated_by <> '');
ALTER TABLE finalized_payroll
    ADD CONSTRAINT finalized_payroll_finalized_by_is_attributed CHECK (finalized_by <> '');
ALTER TABLE reversal
    ADD CONSTRAINT reversal_reversed_by_is_attributed CHECK (reversed_by <> '');

-- The log's own row: the actor, and the target the entry is about. A blank
-- target_type or target_id is an entry pointing at nothing.
ALTER TABLE action_log_entry
    ADD CONSTRAINT action_log_entry_actor_is_attributed CHECK (actor <> '');
ALTER TABLE action_log_entry
    ADD CONSTRAINT action_log_entry_names_its_target
    CHECK (target_type <> '' AND target_id <> '');
