# A Person is Employer-scoped, not global

`CONTEXT.md` defines a Person as "a human being, independent of any job they hold". The `person` table Salt actually stores carries an `employer_id`, and the same human employed by two Employers is two rows. This is a deliberate narrowing of the domain term, recorded so a future reader does not read it as an oversight.

Salt has no way to know that two rows are the same human. There is no national-identifier matching, no identity resolution, and — more decisively — no consent story under which one Employer's personal data about a human becomes visible to another. A global Person would be a claim Salt cannot substantiate, and the claim it would accidentally make is precisely the one tenant isolation exists to prevent: that this Employer may learn where else this human works. Two rows asserting nothing is safer than one row asserting something false.

It also keeps a rule total that would otherwise need an exception. Every read function in `payroll-app` takes `employer_id` and filters on it (ADR-0017); a global `person` table would be the single entity that could not, and the one place a cross-Employer leak could hide.

## Consequences

- The glossary entry for Person now states the narrowing. The domain concept is unchanged; what Salt *records* is a Person as known to one Employer.
- Duplicate humans across Employers are expected and are not a data-quality defect to be cleaned up.
- If Salt ever needs one human across Employers — a cross-employer tax view, say — that is a new concept with its own consent story, not a widening of this table. Merging these rows retroactively would be expensive, which is why the decision is recorded rather than drifted into.
- Person carries one `full_name` and nothing else. Date of birth, tax number, address and bank details are not stored, because payroll may want them one day and "one day" is not a reason.
