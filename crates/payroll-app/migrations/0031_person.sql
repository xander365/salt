-- Person (issue #51, ADR-0020): a human known to one Employer, never a
-- global table. Created only as the side effect of creating an Employment
-- (`payroll_app::create_employment`) — there is no route, and no use case,
-- that creates a Person on its own.
CREATE TABLE person (
    id          TEXT PRIMARY KEY,
    employer_id TEXT NOT NULL REFERENCES employer (id),
    full_name   TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by  TEXT NOT NULL
);

ALTER TABLE person
    ADD CONSTRAINT person_full_name_is_not_blank
    CHECK (btrim(full_name) <> '' AND full_name ~ '[^[:space:]]');
ALTER TABLE person
    ADD CONSTRAINT person_created_by_names_an_actor
    CHECK (btrim(created_by) <> '');

ALTER TABLE employment
    ADD CONSTRAINT employment_person_id_references_person
    FOREIGN KEY (person_id) REFERENCES person (id);

-- Restating the grant set, as every migration since 0017 does after a
-- schema change: `GRANT ... ON ALL TABLES` only covers the tables that
-- exist when it runs, so restating keeps the whole matrix, `person`
-- included, provably intended rather than merely inherited. `person` is
-- append-only, the same as `finalized_payroll`, `reversal` and
-- `action_log_entry`: no use case here ever corrects a `full_name` once
-- written (Deep Instructions), so there is no UPDATE grant to narrow away
-- later — a future rename ticket adds it back deliberately.
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO payroll_app;
REVOKE UPDATE, DELETE ON finalized_payroll, reversal, action_log_entry, person FROM payroll_app;
REVOKE DELETE ON operator, employer_membership FROM payroll_app;
