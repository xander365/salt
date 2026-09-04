-- Person (issue #51, ADR-0020): a human known to one Employer, never a
-- global table. Created only as the side effect of creating an Employment
-- (`payroll_app::create_employment`) — there is no route, and no use case,
-- that creates a Person on its own.
CREATE TABLE person (
    id          TEXT PRIMARY KEY,
    employer_id TEXT NOT NULL REFERENCES employer (id),
    full_name   TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by  TEXT NOT NULL,
    -- `id` remains globally unique, but this second key is what lets the
    -- Employment foreign key below prove that its Employer and Person agree.
    CONSTRAINT person_id_is_unique_within_employer UNIQUE (employer_id, id)
);

ALTER TABLE person
    ADD CONSTRAINT person_full_name_is_not_blank
    CHECK (btrim(full_name) <> '' AND full_name ~ '[^[:space:]]');
ALTER TABLE person
    ADD CONSTRAINT person_created_by_names_an_actor
    CHECK (btrim(created_by) <> '');

-- Existing Employments predate `person`, so their old `person_id` values
-- have no row for the new foreign key to reference. The old schema did not
-- retain a full name, and an id could have occurred under more than one
-- Employer. Backfill one visibly-marked Person per `(employer_id, person_id)`
-- and replace each legacy id with a stable, scoped one before adding the
-- constraint. This keeps the upgrade lossless while making clear that a
-- later name-correction capability is needed for these legacy records.
INSERT INTO person (id, employer_id, full_name, created_at, created_by)
SELECT DISTINCT ON (employer_id, person_id)
       'legacy-person-' || md5(format(
           '%s:%s%s:%s',
           octet_length(employer_id), employer_id,
           octet_length(person_id), person_id
       )),
       employer_id,
       'Legacy Person ' || person_id,
       created_at,
       created_by
FROM employment
ORDER BY employer_id, person_id, created_at, id;

UPDATE employment
SET person_id = 'legacy-person-' || md5(format(
    '%s:%s%s:%s',
    octet_length(employer_id), employer_id,
    octet_length(person_id), person_id
));

-- The two columns are one relationship: a Person may only name an
-- Employment of that Person's Employer. Referencing both makes a cross-
-- Employer association unrepresentable, rather than depending on each
-- caller to perform the scope check correctly.
ALTER TABLE employment
    ADD CONSTRAINT employment_person_is_scoped_to_its_employer
    FOREIGN KEY (employer_id, person_id) REFERENCES person (employer_id, id);

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
