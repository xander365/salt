# Historical ActionLog actor text is frozen; new entries carry an OperatorId

`payroll-app` takes the actor as free text and always has. Now that Salt has authenticated Operators, `salt-server` passes `operator:<OperatorId>` — derived from the session, never from the request body. Existing rows are left exactly as they are. The column stays `text` and never becomes a foreign key to `operator`.

An id rather than an email, because emails change and payroll history must not become wrong when someone marries. Frozen rather than migrated, because the alternative is inventing an Operator row to represent a string that was written under a different regime, and then a foreign key asserting a relationship that did not exist when the row was written. That is rewriting history to make a schema tidier — the thing every other decision in this codebase refuses (ADR-0004, ADR-0006, ADR-0011).

## Consequences

- A reader of the ActionLog must expect two shapes of actor value and can tell them apart by the `operator:` prefix.
- Nothing joins `action_log_entry.actor` to `operator`. Naming an Operator in a UI is a lookup the reader does deliberately, and it can fail for old rows — correctly, because those rows do not name an Operator.
- The actor is never accepted from an HTTP request body. A forged `"actor"` field has no route that reads it.
