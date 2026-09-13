-- Issue #79, hardened: a StandingPayItem is a stated fact a proposal copies
-- verbatim, so what it holds and when it began must never change under the
-- runs already proposed from it. Ending it is the only revision there is
-- (`end_standing_pay_item`), and ending writes exactly three columns. The
-- restricted role therefore gets UPDATE on those three and nothing else —
-- the same column-scoped grant migration 0031 gives `person.full_name` —
-- so rewriting an item's amount, label, Employment or effective-from date
-- in place is refused by the database, not merely avoided by the code.
--
-- A changed amount is a new item, recorded from a PayPeriod start, with the
-- old one ended beside it.

-- Restate the restricted role's permission matrix, as every migration since
-- 0017 does.
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO payroll_app;
REVOKE UPDATE, DELETE ON finalized_payroll, reversal, action_log_entry, person, standing_pay_item FROM payroll_app;
REVOKE DELETE ON operator, employer_membership FROM payroll_app;
GRANT UPDATE (full_name) ON person TO payroll_app;
GRANT UPDATE (ended_at, ended_by, ended_reason) ON standing_pay_item TO payroll_app;
