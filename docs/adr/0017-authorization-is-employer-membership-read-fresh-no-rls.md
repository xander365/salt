# Authorization is EmployerMembership, read fresh per request, with no Row Level Security

Every Employer-scoped route resolves an `AuthorizedEmployerContext` from one joined query — session, Operator status and EmployerMembership together — on every request, with no caching. Handlers take the EmployerId out of that context and never from the URL path, so a route that forgets to authorize does not compile. We rejected adding PostgreSQL Row Level Security on top.

Reading membership fresh is what makes revocation immediate: an Operator whose membership is removed is refused on their next click, with no invalidation mechanism to design. It costs one indexed query per request.

RLS is the tempting second layer, and payroll data is sensitive enough that refusing it needs a reason. RLS defends against a *second* path to the database. Salt has one: `salt-server`, which writes no SQL at all (ADR-0018). The policy would restate the application's rule in a second language, and the session variable the policy reads would be set by the very code the policy is supposed to defend — so a bug in that code defeats both layers at once. The genuine second layer is cheaper and does not have that flaw: every read function in `payroll-app` takes `employer_id` explicitly and filters on it in SQL, so a bug in the extractor still cannot return another Employer's row.

## Consequences

- A resource id never authorizes. Every Employer-scoped URL carries the EmployerId, including nested resources, so the extractor has one on every protected route without exception.
- A request for a resource outside the caller's authorized Employers returns **404**, not 403 — 403 would confirm that the id exists, which is a free existence oracle over payroll. 403 is reserved for the case where the caller *is* a member but their role is too low, where it leaks nothing new.
- RLS is revisited if and when a second writer to the database appears.
