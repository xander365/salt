# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

Payroll Operators at Namibian small and medium employers: humans who sign into Salt and run one Employer's payroll, holding either the Owner role (full access, including Employer configuration and membership) or the PayrollOperator role (runs payroll only). They work at a desk, at a laptop, repeatedly and often under time pressure at month-end, moving between a People list, a monthly payroll worksheet, and finalized-payroll records. Most operators belong to exactly one Employer.

## Product Purpose

Salt is payroll software for Namibian SMEs: it calculates, explains, and permanently records what an Employer pays a Person for a pay period, under the statutory rules in force at that time. Success is an Operator completing a month's payroll end to end, from a browser, with figures that reconcile across every output (worksheet, payslip, register, payment summary) and no developer intervention.

## Positioning

Salt separates statutory law (`StatutoryRule`) from its own provisional choices (`SaltPolicy`) everywhere the law is silent, and never lets an operator or a screen blur the two. Every finalized payroll is immutable and self-explaining — it never looks back at today's master data to justify a historical figure. A neighboring generic payroll tool cannot make either claim truthfully.

## Operating Context

- Monthly pay frequency only (weekly/fortnightly are refused, not silently unsupported).
- Namibian tax year: 1 March to end of February.
- Master data (`EmployerParticulars`, `PersonParticulars`, `CompensationTerms`) is correctable with a stated reason but is never a mutable "current" field, and freezes into every `FinalizedPayroll`.
- React/the browser owns no payroll arithmetic — all figures (PAYE, net pay, totals) come from the server.
- The current milestone is an internal prototype: two consecutive months, end to end, on fictional data. Statutory reporting (ETX, PAYE5, SSC, VET, ECF) is out of scope. Overtime, standing pay items, and corrections are in scope for the surrounding milestone; hourly-paid employment and leave are explicitly out.

## Capabilities and Constraints

- Design system: shadcn/ui with Tailwind (decided; Mantine considered and rejected). The visual direction lives in CSS variables so a later revision is a token swap.
- Navigation: persistent People / Payroll / Employer navigation on every authenticated screen, with the active Employer always named. No Reports tab exists — reports are not built yet, and a door that does not open is not drawn.
- An Operator with exactly one Employer meets no employer-selection step; switching Employer clears cached data.
- Money is a whole number of cents on the wire (never a float), shown right-aligned in tabular numerals with N$ shown where needed. Dates are unambiguous and human-readable.
- Accessibility: visible keyboard focus, every control labelled, no error relies on colour alone.
- A failed request preserves typed data where safe and offers a truthful save-and-retry state; there is no offline payroll processing.
- Dark mode, animation-heavy effects, and a theme editor are explicitly deferred.
- Terminology throughout the product follows `CONTEXT.md` exactly (e.g. "Employer" not "company/tenant", "Operator" not "user/account", "Employment" not "employee/staff member").

## Brand Commitments

- Product name: Salt.
- Visual direction (owner-confirmed, `docs/domain/Salt-Customer-Readiness-Grill-Brief.md` D4/D9/D13, plus this ticket's own interview): a calm, professional, light interface; restrained neutral base with a deep blue accent reserved for primary actions, active navigation, and links; comfortable, roomy density (not maximally compact); strong typographic hierarchy; compact, readable tables. One direction only — no concept exploration round.

## Evidence on Hand

None yet as real customer data. The prototype fixture is five fictional employees at one Employer, covering plain salary, a standing taxable allowance, overtime plus a standing deduction, a mid-year joiner, and a leaver (`docs/domain/Salt-Customer-Readiness-Grill-Brief.md` D21). Do not fabricate real customer names, testimonials, or figures anywhere in the product.

## Product Principles

1. The server is the only source of payroll truth — the browser renders and explains, it never calculates.
2. A finalized record explains itself forever; nothing about it is ever rebuilt from what master data says today.
3. Statutory law and Salt's own provisional policy are never presented as the same kind of claim.
4. Absence of a record is never treated as an answer — a resolved period always has an affirmative reason.
5. One visual direction, pinned in tokens, so revising the look is a token swap rather than a redesign.

## Accessibility & Inclusion

Visible keyboard focus on every interactive element, a labelled form control for every input, and no error state that relies on colour alone to be understood — required by this ticket's acceptance criteria, not yet independently audited against a formal standard.
