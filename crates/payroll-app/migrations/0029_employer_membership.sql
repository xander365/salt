-- EmployerMembership (issue #43, parent #38): grants one Operator access to
-- one Employer with a role. Membership *is* the authorization decision
-- (ADR-0017) — holding an EmployerId grants nothing without a row here
-- naming it. One Operator may hold several memberships, one per Employer,
-- which is what the primary key below enforces.
--
-- Both roles are modelled now, not just the one route-relevant distinction
-- Specs 1-3 need: retrofitting a role column onto live memberships later is
-- far more expensive than carrying the enum from the start (§0.6, §0.39).
--
-- A membership is revoked by `status`, never a delete, the same discipline
-- migration 0028 applies to a disabled Operator: the grant and its later
-- revocation are both facts an audit trail should keep naming.
--
-- Ships no administrative API and no HTTP route (§0.39) — the only caller
-- through Spec 3 is bootstrap's first Owner membership.
CREATE TABLE employer_membership (
    operator_id UUID NOT NULL REFERENCES operator (id),
    employer_id TEXT NOT NULL REFERENCES employer (id),
    role        TEXT NOT NULL CHECK (role IN ('owner', 'payroll_operator')),
    status      TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'revoked')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (operator_id, employer_id)
);

-- Restating the grant set, as every migration since 0017 does after a
-- schema change: `GRANT ... ON ALL TABLES` only covers the tables that exist
-- when it runs, so restating keeps the whole matrix, `employer_membership`
-- included, provably intended rather than merely inherited.
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO payroll_app;
REVOKE UPDATE, DELETE ON finalized_payroll, reversal, action_log_entry FROM payroll_app;
REVOKE DELETE ON operator FROM payroll_app;
-- A membership is revoked by `status`, never removed — the same reasoning
-- migration 0028 states for `operator`, applied to the row that is this
-- ticket's own authorization decision.
REVOKE DELETE ON employer_membership FROM payroll_app;
