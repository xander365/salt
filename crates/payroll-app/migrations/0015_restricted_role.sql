-- The restricted application role (§6.2, §11). It is NOLOGIN: nothing ever
-- authenticates as this role directly. A login role that needs the app's
-- permission set reaches it with `SET ROLE payroll_app`, which is also how
-- a test proves the refusal — proving it as the owner role would pass while
-- proving nothing (see the ticket's own instruction to that effect).
--
-- Guarded so this migration is safe to run against any database in the
-- cluster: roles are cluster-wide, not per-database, and #[sqlx::test]
-- applies every migration again to a fresh throwaway database per test —
-- concurrently, so an EXISTS check alone still races two such migrations
-- against the same "CREATE ROLE"; catching the failure is what actually
-- closes the race, not the check.
DO $$
BEGIN
    BEGIN
        CREATE ROLE payroll_app NOLOGIN;
    EXCEPTION
        WHEN duplicate_object OR unique_violation THEN
            NULL;
    END;
END
$$;

-- Lets whichever role applied this migration SET ROLE to payroll_app
-- afterwards, without granting login to payroll_app itself.
GRANT payroll_app TO CURRENT_USER;

GRANT USAGE ON SCHEMA public TO payroll_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO payroll_app;

-- INV-004 as a database permission, not application discipline: immutability
-- no longer depends on every call site remembering not to touch this table.
REVOKE UPDATE, DELETE ON finalized_payroll FROM payroll_app;
