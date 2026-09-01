# Operators authenticate with opaque server-side sessions, not JWT

Salt's first browser client authenticates with a random token stored server-side in PostgreSQL and carried in an `HttpOnly; Secure; SameSite=Lax` cookie. Only a hash of the token is stored, so a database leak is not a set of live logins. We rejected JWT, which is the default assumption for a JSON API and therefore the decision most likely to be re-litigated.

JWT's advantage is that a server can validate a token without touching storage. Salt has no problem that advantage solves: one server process, one database, no third-party consumers, no horizontal scale in sight. Its costs are real and immediate — logout needs a revocation list (which is server-side session state, arrived at by a longer road), a disabled Operator keeps working until expiry, a membership change needs an expiry dance, and the browser must hold the credential somewhere JavaScript can reach it. An opaque session makes logout a `DELETE`, makes revocation take effect on the next request, and puts the credential where an XSS cannot read it.

## Consequences

- Every authenticated request reads the database (see ADR-0017). This is deliberate and is what makes revocation immediate.
- Sessions carry both an idle timer (8 hours) and an absolute timer (12 hours). Many concurrent sessions per Operator are allowed; logout deletes the row rather than marking it revoked, because a session is not history — the ActionLog is.
- The `Secure` cookie attribute is on by default and can only be disabled by an explicit development flag that the server refuses to accept together with production mode. Default-off would be one forgotten environment variable away from shipping cookies in the clear.
- If Salt later grows a client Salt does not deploy, this decision is revisited then — not pre-emptively.
