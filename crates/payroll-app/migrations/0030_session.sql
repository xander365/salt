-- Session (issue #44, parent #38): how a signed-in Operator stays signed
-- in. `token_hash` is the only thing the database ever holds of the
-- token — a SHA-256 hex digest, since the token is already at least 256
-- bits from the operating system's cryptographic random source and a slow
-- password KDF here would cost a hash per request for no benefit (ADR-0016).
--
-- `expires_at` is the absolute timer, written once at creation as
-- `created_at + 12 hours` and never moved: `payroll-app` computes it in
-- Rust rather than restating "12 hours" a second time as a generated
-- column, so the duration exists in exactly one place
-- (`payroll_app::session::absolute_timeout`). The idle timer is not a
-- stored column at all — `last_seen_at + 8 hours` is checked live by
-- whichever query loads the session, which is what lets `last_seen_at`
-- advance without ever touching `expires_at`.
--
-- A session is deleted, never revoked by status: unlike an Operator or an
-- EmployerMembership there is nothing here an audit trail needs to keep
-- naming once a session's timers have passed, so the restricted role keeps
-- its DELETE grant on this table alone.
CREATE TABLE session (
    id           UUID PRIMARY KEY,
    operator_id  UUID NOT NULL REFERENCES operator (id),
    token_hash   TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL,
    last_seen_at TIMESTAMPTZ NOT NULL,
    expires_at   TIMESTAMPTZ NOT NULL,
    CONSTRAINT session_last_seen_at_is_not_before_created_at
        CHECK (last_seen_at >= created_at),
    CONSTRAINT session_expires_at_is_after_created_at
        CHECK (expires_at > created_at)
);

-- The index a token lookup runs against. UNIQUE because two sessions
-- hashing to the same value would mean the 256-bit token space collided —
-- cryptographically negligible, but the index may as well say so rather
-- than silently allow it.
CREATE UNIQUE INDEX session_token_hash_key ON session (token_hash);

-- Never human-typed, so the simple guard already used for other
-- never-typed columns (migration 0028's `password_verifier`) is enough.
ALTER TABLE session
    ADD CONSTRAINT session_token_hash_is_not_blank
    CHECK (btrim(token_hash) <> '');

-- Restating the grant set, as every migration since 0017 does after a
-- schema change: `GRANT ... ON ALL TABLES` only covers the tables that
-- exist when it runs, so restating keeps the whole matrix, `session`
-- included, provably intended rather than merely inherited.
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO payroll_app;
REVOKE UPDATE, DELETE ON finalized_payroll, reversal, action_log_entry FROM payroll_app;
REVOKE DELETE ON operator, employer_membership FROM payroll_app;
