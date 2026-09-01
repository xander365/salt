-- Operator (issue #41, parent #38): one global human identity Salt can name,
-- separate from every Employer-scoped fact the rest of this schema
-- describes. This migration creates the identity and its credential only;
-- authorizing an Operator against one Employer is a later ticket's table.
--
-- Email is stored exactly as entered, so an Operator's own capitalisation
-- survives, but the UNIQUE index below is on the folded (lower-cased) form,
-- not the column itself — 'Alice@x' and 'alice@x' collide even though the
-- stored text differs. Enforcing this in the database, not in application
-- discipline, is the only way two concurrent inserts of the same folded
-- email can be made to resolve to one row rather than two.
--
-- The password itself is never stored. `password_verifier` holds an
-- Argon2id PHC string (`$argon2id$v=19$m=...,t=...,p=...$salt$hash`), which
-- carries its own hashing parameters inline. That is what lets this
-- application's default Argon2 parameters change later while every row
-- hashed under the old ones still verifies — there is no separate
-- parameters column to keep in step with it.
--
-- `failed_attempt_count` and `first_failure_at` are columns this migration
-- ships for a lockout rule a later ticket implements (issue #41's own
-- instruction); nothing in this one reads or enforces them.
CREATE TABLE operator (
    id                   UUID PRIMARY KEY,
    email                TEXT NOT NULL,
    display_name         TEXT NOT NULL,
    password_verifier    TEXT NOT NULL,
    status               TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled')),
    failed_attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (failed_attempt_count >= 0),
    first_failure_at     TIMESTAMPTZ,
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX operator_email_folded_key ON operator (lower(email));

-- `btrim` alone leaves a tab- or newline-only value passing (PostgreSQL's
-- single-argument `btrim` trims spaces only), so a human-typed column gets
-- both halves, the same guard migration 0027 put on an Employer's name.
ALTER TABLE operator
    ADD CONSTRAINT operator_email_is_not_blank
    CHECK (btrim(email) <> '' AND email ~ '[^[:space:]]');
ALTER TABLE operator
    ADD CONSTRAINT operator_display_name_is_not_blank
    CHECK (btrim(display_name) <> '' AND display_name ~ '[^[:space:]]');

-- Folding case is not enough on its own to make two emails that a person
-- reads as one collide: ' alice@x' and 'alice@x' fold to different strings,
-- so without this the unique index above would happily admit both and Salt
-- would hold two accounts distinguishable only by characters nobody can
-- see. `create_operator` trims before it writes; this constraint is what
-- makes that true of every row rather than of the rows one function wrote.
-- Stated as a leading/trailing whitespace refusal rather than
-- `email = btrim(email)` because PostgreSQL's single-argument `btrim` would
-- again miss a leading tab.
ALTER TABLE operator
    ADD CONSTRAINT operator_email_has_no_surrounding_whitespace
    CHECK (email !~ '^[[:space:]]' AND email !~ '[[:space:]]$');
ALTER TABLE operator
    ADD CONSTRAINT operator_display_name_has_no_surrounding_whitespace
    CHECK (display_name !~ '^[[:space:]]' AND display_name !~ '[[:space:]]$');

-- Never human-typed — this column only ever holds what `create_operator`
-- writes — so the simpler guard already used for attribution columns
-- (migration 0023) is enough.
ALTER TABLE operator
    ADD CONSTRAINT operator_password_verifier_is_not_blank
    CHECK (btrim(password_verifier) <> '');

-- Restating the grant set, as every migration since 0017 does after a
-- schema change: `GRANT ... ON ALL TABLES` only covers the tables that exist
-- when it runs, so restating keeps the whole matrix, `operator` included,
-- provably intended rather than merely inherited.
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO payroll_app;
REVOKE UPDATE, DELETE ON finalized_payroll, reversal, action_log_entry FROM payroll_app;
-- An Operator is disabled by `status`, never removed (§6). A deleted row
-- would take an id out of the world that the audit trail still names, and
-- ADR-0019 already refuses to rewrite history to keep a schema tidy. `UPDATE`
-- stays, because disabling is an update; only the erasure is taken away, so
-- the rule is a permission rather than a convention the next use case could
-- forget.
REVOKE DELETE ON operator FROM payroll_app;
