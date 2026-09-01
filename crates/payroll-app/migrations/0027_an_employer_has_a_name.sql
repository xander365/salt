-- An Employer gains one human-readable name (issue #40), so the application
-- has something to show a person other than an id. It is written once, at
-- creation, by `create_employer`; the Owner capability that would let it
-- change later is deferred (§0.39), so this migration ships no rename path.
--
-- `NOT NULL` with no DEFAULT is safe here: the `employer` table carries no
-- seed rows, so every existing row a real deployment could have is one this
-- build already requires a name for.
--
-- Blank and whitespace-only are both refused, the same guard 0019 and 0023
-- put on every attribution column: a name of ' ' shows a person nothing,
-- and it would be unfixable after the fact for the same reason those columns
-- are — nothing about payroll reads this column, but the person looking at
-- the Employer list still would.
ALTER TABLE employer ADD COLUMN name TEXT NOT NULL;
ALTER TABLE employer ADD CONSTRAINT employer_name_is_not_blank CHECK (btrim(name) <> '');

-- Restating the grant set, as 0017 does after every schema change: `GRANT
-- ... ON ALL TABLES` only covers the tables that exist when it runs, and
-- `payroll_app` never gets a per-column grant of its own to reach for
-- instead — restating keeps the whole matrix, this new column included,
-- provably intended rather than merely inherited.
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO payroll_app;
REVOKE UPDATE, DELETE ON finalized_payroll, reversal, action_log_entry FROM payroll_app;
