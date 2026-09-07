---
version: 1
slug: "app-shell"
primary_target: "app-shell"
related_targets: ["web/src/App.tsx"]
---

## Direction contract

THESIS: Salt is an Operate-mode payroll tool an Operator returns to daily; the shell's one job is to keep People, Payroll and the active Employer always in view and never make figures compete with decoration for attention. Refuses the category-default dashboard-of-widgets home screen.

OWN-WORLD: shadcn/ui (new-york) on Tailwind v4. True neutral grays (oklch, chroma 0) for background/text/borders; one deep-blue primary (oklch(0.398 0.153 260.5)) reserved for primary buttons, the active nav item, and links. Comfortable spacing (p-6/gap-6 cards, generous form gaps). System sans-serif stack, no display face. Money: `N$1,234.56`, right-aligned, tabular-nums. Dates: `7 Sep 2026`.

STORY: An Operator signs in, lands inside the one Employer they belong to (or picks one), and always sees that Employer's name plus People/Payroll navigation, wherever they go. They complete a task (add a person, run payroll) with the next action always ahead of any decoration.

FIRST VIEWPORT: A thin header bar — Employer name (linked, level-1 heading) and role badge at left, Switch-employer link (multi-Employer only) and Sign out at right — with a People/Payroll nav row beneath it, a hairline border, then the page content in a max-w-6xl column with comfortable padding.

FORM: Pinned by docs/domain/Salt-Customer-Readiness-Grill-Brief.md D4/D9/D13 (shadcn/ui+Tailwind, one direction, People/Payroll/Employer nav) and the user's own confirmation (deep blue accent, comfortable density) during issue #88's implementation. No concept-seed round run — the world was already settled, not open.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance.
