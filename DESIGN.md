# Design

<!-- Written from the shipped world (issue #88), not before it. -->

This is Salt's one visual direction (`docs/domain/Salt-Customer-Readiness-Grill-Brief.md` D4/D9/D13). It exists so a later revision is a token swap, not a redesign — do not hard-code a colour, radius, or spacing value inside a component; add or change a token in `web/src/index.css` instead.

## Audience and task flow

The audience is a payroll Operator at a Namibian SME (see `PRODUCT.md`), signed in and working inside one Employer at a time, most often with exactly one Employer to their name. The task is repetitive and month-end-driven: open People, record what someone is paid; open Payroll, run and finalize a period; occasionally reopen a finalized payroll to explain a figure. The interface's one job is to keep that path short and the current task in front of decoration — there is no dashboard of widgets, and the Employer landing screen is two sentences, not a grid of cards.

Every authenticated screen renders beneath `EmployerShell` (`web/src/routes/EmployerShell.tsx`), so it always carries, in this order: the active Employer's name (a level-1 heading, linked back to that Employer's own Overview — the "Employer" third of D13's People / Payroll / Employer navigation), that Employer's role badge, People and Payroll as persistent nav links, a "Switch employer" link (only when the Operator has more than one Employer — never a switcher widget; issue #61's own decision), and Sign out.

## Stack

shadcn/ui, `new-york` style, on Tailwind v4 (`@tailwindcss/vite`), installed under `web/`. Components live in `web/src/components/ui/` (only the primitives actually used — `button`, `input`, `label`, `card`, `badge` — ship here; add more with `npx shadcn@latest add <name>` as a real screen needs one, never speculatively). `web/src/lib/utils.ts` carries the `cn` helper. The `@/*` import alias resolves to `web/src`.

## Tokens

All of it lives as CSS custom properties in `web/src/index.css`, under `:root`, re-exposed to Tailwind's utilities via `@theme inline`. There is one theme — dark mode is deferred (issue #88's own Deep Instructions) and there is deliberately no `.dark` block yet.

**Colour** — true, uncoloured neutrals (`oklch(... 0 0)`, zero chroma) plus one deliberate accent:

| Token | Value | Use |
|---|---|---|
| `--background` / `--card` / `--popover` | `oklch(1 0 0)` | page and surface ground |
| `--foreground` / `--card-foreground` | `oklch(0.145 0 0)` | body text |
| `--muted-foreground` | `oklch(0.556 0 0)` | secondary text (labels, captions, timestamps) |
| `--border` / `--input` | `oklch(0.922 0 0)` | hairlines, input borders |
| `--primary` | `oklch(0.398 0.153 260.5)` | the one deep-blue accent: primary buttons, the active nav item, links |
| `--primary-foreground` | `oklch(0.985 0 0)` | text/icons on `--primary` |
| `--secondary` / `--muted` | `oklch(0.97 0 0)` | quiet fills |
| `--accent` | `oklch(0.94 0.017 260.5)` | hover fill, faintly blue-tinted |
| `--destructive` | `oklch(0.577 0.245 27.325)` | errors — always paired with an icon and text, never colour alone |
| `--warning` | `oklch(0.6 0.16 60)` | blocked/standing-fact states |
| `--success` | `oklch(0.48 0.13 152)` | finalized / confirmed states |
| `--ring` | `--primary` at 50% alpha | focus ring |

An earlier draft used Tailwind's "slate" neutrals (a blue-tinted gray) and it read as pervasively blue in the built app — screenshotted and rejected during this ticket. True neutrals plus one accent is what "restrained" (the settled brief's own word) actually looks like; do not reintroduce a tinted neutral scale.

**Radius**: `--radius: 0.5rem`, with `--radius-sm/md/lg/xl` derived from it in `@theme inline`.

**Typography**: system sans-serif stack only (`--font-sans`) — no display or marketing face. This is an Operate-mode admin tool; a workhorse system stack is the correct choice, not a missed opportunity. Headings use `font-semibold tracking-tight`; secondary text uses `text-muted-foreground`.

**Density**: comfortable, not maximally compact (the Operator's own choice during this ticket) — cards pad `p-6`, forms stack fields with `gap-4`, sections stack with `gap-6`. Table-like figure grids (`Figures`, PAYE/SSC workings) are the one place row height tightens, because they carry many short values at once.

## Money and dates

- **Money as data** (a worksheet figure, a table cell, a finalized record) renders through `moneyDisplayText`/`formatMoneyDisplay` (`web/src/money.ts`) or the `<Money>` component (`web/src/components/Money.tsx`): `N$1,234.56`, right-aligned, tabular numerals (the `.money` utility class in `index.css`, or Tailwind's `tabular-nums` + `text-right`).
- **Money echoed from what an Operator just typed** (a form's own save confirmation, e.g. "Pay of 15000.00 recorded from 2026-09-01") keeps the plain decimal `formatCents` gives — no `N$`, no thousands separator. It is a readback of an exact input, not a data display, and reformatting it would make the confirmation look like it recorded a different number than the one typed. An *editable* input's value is always the plain decimal for the same reason: `parseCentsInput` cannot read a comma or a currency symbol back out.
- **Dates shown as data** (a list, a header, a record) render through `humanDate`/`humanDateRange` (`web/src/format.ts`): `7 Sep 2026`. **Dates inside a `type="date"` input**, and dates echoed in a form's own confirmation sentence, keep the browser's ISO value — the same input-vs-display distinction as money.

## Shared states

Seven, under `web/src/components/states/`, used instead of a screen improvising its own:

| Component | Renders as | Demonstrated on |
|---|---|---|
| `LoadingState` | icon + text, `role="status"` | People, Payroll, Employment, PayrollRun, FinalizedPayroll, Workings, session check |
| `EmptyState` | dashed card, icon + sentence | People with no employees, PayrollRuns with none, PayrollRun with no members, no-Employer AppLanding |
| `ValidationError` | icon + red text, `role="alert"`, keeps the `id` a field's `aria-describedby` points at | every form's field-level complaint |
| `BlockedItem` | amber card, icon + sentence + link to the fix | a payroll run member's standing blockers |
| `StaleBanner` | muted card, clock icon | a run mid-refetch, and a member whose earnings were saved after its figures were last calculated |
| `FinalizedBanner` | green card, lock icon | "This payroll is finalized and cannot be changed." |
| `FailedRequestState` | icon + message + a real retry button that repeats the same request | every load/mutation failure that offers a retry |

No error, blocked, or stale state is colour alone — each pairs its colour with an icon and text.

## Forms

- `Label` + `Input` (shadcn) for every text/date/number field; a native `<input type="radio">`/`<input type="checkbox">` styled with `accent-primary` for choices — not a Radix `RadioGroup`, since these are plain, small, same-page choices and the native control is already fully accessible.
- A field-level complaint renders as `<ValidationError id={errorId}>`, and the field itself carries `aria-invalid`/`aria-describedby` pointing at it (`fieldErrorProps`, unchanged from before this ticket).
- A save confirmation is a `role="status"` sentence beneath the form, not a toast — it has to survive being read back by the browser journey and by a screen reader without timing out.
- A failed submission never clears what was typed. The field values stay in state; only a retry (resubmitting the same values) or an edit clears the error.

## Navigation and worksheet

- Persistent People / Payroll nav (`EmployerShell`), active item styled with `--primary`, `aria-current` implicit via `react-router-dom`'s `NavLink`.
- A back-link inside a screen (e.g. Employment's "← People") is given a distinct `aria-label` ("Back to People") from the persistent nav item beside it — two links reading "People" on one screen is an ambiguity a screen reader's link list would surface, even where nothing else collides with it.
- The payroll worksheet (`PayrollRun.tsx`'s `Member`) reserves an **Hours** row (`Hours: —`) between the earnings editor and the blockers list. Hourly pay is out of this milestone; the slot exists so filling it later is not a relayout.

## What is deferred

Dark mode, animation-heavy effects, and a theme editor — not built, and no token above assumes them. When any of the three is scoped, it is a token/variant addition to this file and `index.css`, not a rewrite.
