// One test, not a suite (issue #68's own acceptance criterion). It walks
// the whole journey issue #59 exists for, in a real browser, against a
// built `salt-server` and a real PostgreSQL: sign in, land in the single
// Employer, add an employee, record every fact a payroll needs, run and
// finalize an Ordinary payroll, and reload to prove the figures came from
// the server.
//
// Every element below is found by accessible role and name — never by CSS
// class, DOM structure or a test-only attribute (issue #68's own acceptance
// criterion). Nothing here is mocked, intercepted or stubbed: every click
// crosses the real network to the real router this suite's `webServer`
// started.
//
// Prior art is `crates/salt-server/tests/payroll_journey.rs`'s own
// `signing_in_and_running_one_ordinary_payroll_end_to_end` — the same
// journey, one layer lower, over HTTP in-process rather than a browser.
// Where the two disagree about what the journey is, that one is right and
// this one is wrong (issue #68's own Deep Instructions). This test asserts
// less than that one does on purpose: everything a plain HTTP call could
// already prove — malformed bodies, cross-Employer 404s, every declared
// route — is that test's job, not this one's (§0.37, the same issue's own
// Deep Instructions).

import { readFile } from 'node:fs/promises';
import { type Page, expect, test } from '@playwright/test';
import { type BootstrappedOperator, CREDENTIALS_PATH } from '../global-setup.js';

/** The nine figures §0.29 names and the accessible names the screen gives
 * them. Order is immaterial: each value is read by what a person calls it,
 * never by where its definition happens to sit in the document. */
const FIGURE_FIELDS = [
  { field: 'basicPayCents', name: 'Basic Pay' },
  { field: 'taxableAllowancesCents', name: 'Taxable Allowances' },
  { field: 'grossCents', name: 'Gross' },
  { field: 'taxableRemunerationCents', name: 'Taxable Remuneration' },
  { field: 'payeCents', name: 'PAYE' },
  { field: 'employeeSscCents', name: 'Employee SSC' },
  { field: 'employerSscCents', name: 'Employer SSC' },
  { field: 'totalDeductionsCents', name: 'Total Deductions' },
  { field: 'netCents', name: 'Net' },
] as const;

type Figures = Record<(typeof FIGURE_FIELDS)[number]['field'], number>;

/** `web/src/employments/useEmployments.ts`'s own `useCreateEmployment` sets
 * a new Employment's `startDate` to the real "today" — there is no field on
 * the People screen to choose another one (§0's own design: adding an
 * employee never backdates a hire). Every date this journey records or
 * pays therefore has to be built from the same real "today" the running
 * server sees, never a fixed historical one the way
 * `payroll_journey.rs`'s own `january_period()` can afford to be. */
function isoDate(date: Date): string {
  const year = String(date.getFullYear()).padStart(4, '0');
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

/** Mirrors `web/src/taxYear.ts`'s own `currentTaxYearStartingYear`: the tax
 * year the PriorEmploymentForm's own default already assumes, so this test
 * can assert its confirmation sentence without touching that field at
 * all. */
function currentTaxYearStartingYear(today: Date): number {
  return today.getMonth() + 1 >= 3 ? today.getFullYear() : today.getFullYear() - 1;
}

const today = new Date();
// The whole calendar month "today" falls in: this Employer's own
// last-day-of-month schedule generates it as one period, and it is the
// only period this brand new Employment can be run for at all without an
// OpeningBalance (out of scope — Deep Instructions name the steps this
// journey walks, in order, and that is not one of them): the *next*
// month's period would leave this one before it an unresolved preceding
// period, which `finalize` refuses outright
// (`preceding_period_unresolved`).
//
// Starting mid-month, the way every new hire through this screen does
// (`web/src/employments/useEmployments.ts`'s own `useCreateEmployment`
// always dates `startDate` to today), also means
// `crates/payroll/src/calculation.rs`'s own joiner proration (PC-005)
// divides `basicPayCents` by however many of this period's days are left —
// a fraction only the real calendar decides, and never this test's to
// reimplement (§0's own instruction that no arithmetic belongs outside the
// server applies here just as much as it does to `web/src/routes`). The
// figures this test reads are asserted as relationships for exactly that
// reason, the same way `payroll_journey.rs`'s own two summed figures are.
const periodStart = new Date(today.getFullYear(), today.getMonth(), 1);
const periodEnd = new Date(today.getFullYear(), today.getMonth() + 1, 0);
const payDate = new Date(today.getFullYear(), today.getMonth() + 1, 5);
const periodStartText = isoDate(periodStart);
const periodEndText = isoDate(periodEnd);
const payDateText = isoDate(payDate);
// From `periodStart`, not `today`: right at the tax year's own March
// boundary the two can name different tax years, and it is the period's
// own tax year the server checks PriorEmployment's declaration against.
const taxYearStarting = currentTaxYearStartingYear(periodStart);

/** The inverse of `web/src/money.ts`'s own `formatCents`: an exact
 * "whole.dd" string back into a whole number of cents, in integer
 * arithmetic only, for the same reason the source it mirrors never lets a
 * `Money` value near a float. */
function parseCentsText(text: string): number {
  const match = /^(\d+)\.(\d{2})$/.exec(text.trim());
  if (match === null) {
    throw new Error(`not a cents-exact amount: "${text}"`);
  }
  const [, whole, fraction] = match;
  return Number(BigInt(whole) * 100n + BigInt(fraction));
}

/** Reads the nine figures §0.29 names off whichever screen is showing them
 * — a payroll run's own calculated member, or a finalized payroll — both of
 * which share this one `Figures` component. Waits for exactly nine to
 * exist first: a bare read the instant Calculate is clicked would otherwise
 * race the response and see none. */
async function readFigures(page: Page): Promise<Figures> {
  const definitions = page.getByRole('definition');
  await expect(definitions).toHaveCount(FIGURE_FIELDS.length);
  const figures = {} as Figures;
  for (const { field, name } of FIGURE_FIELDS) {
    // By the name a person reads beside the amount, never by position in the
    // list. The name is on the group and not on the `definition` itself
    // because ARIA 1.2 prohibits naming a `definition` — see
    // `web/src/routes/payroll/Figures.tsx`.
    const figure = page.getByRole('group', { name, exact: true });
    await expect(figure).toBeVisible();
    const definition = figure.getByRole('definition');
    await expect(definition).toBeVisible();
    figures[field] = parseCentsText(await definition.innerText());
  }
  return figures;
}

test('signing in and running one ordinary payroll end to end', async ({ page }) => {
  const operator: BootstrappedOperator = JSON.parse(await readFile(CREDENTIALS_PATH, 'utf-8'));

  // Sign in.
  await page.goto('/login');
  await page.getByLabel('Email').fill(operator.email);
  await page.getByLabel('Password').fill(operator.password);
  await page.getByRole('button', { name: 'Sign in' }).click();

  // One membership: land in the single Employer, never a switcher screen.
  await expect(page.getByRole('heading', { level: 1, name: operator.employerName })).toBeVisible();

  // Add an employee by full name.
  await page.getByRole('link', { name: 'People' }).click();
  await page.getByLabel('Full name').fill('Ada Lovelace');
  await page.getByRole('button', { name: 'Add employee' }).click();
  await expect(page.getByRole('status')).toHaveText('Added Ada Lovelace.');
  await page.getByRole('link', { name: 'Ada Lovelace' }).click();
  await expect(page.getByRole('heading', { level: 2, name: 'Ada Lovelace' })).toBeVisible();

  // Record CompensationTerms, effective from the start of the period this
  // Employment can first be paid for.
  const payRegion = page.getByRole('region', { name: 'Pay', exact: true });
  await payRegion.getByLabel('Effective from').fill(periodStartText);
  await payRegion.getByLabel('Basic pay').fill('15000.00');
  await payRegion.getByRole('button', { name: 'Save pay' }).click();
  await expect(page.getByText(`Pay of 15000.00 recorded from ${periodStartText}.`)).toBeVisible();

  // Declare PriorEmployment: no prior employment this tax year. Its own
  // default assumes the tax year real "today" falls in, which right at the
  // tax year's own March boundary can differ from the period's — so this
  // fills it explicitly rather than trust that default.
  const priorEmploymentRegion = page.getByRole('region', {
    name: 'Prior employment',
    exact: true,
  });
  const priorEmploymentGroup = priorEmploymentRegion.getByRole('radiogroup', {
    name: 'Did this employee have taxable employment earlier this tax year?',
  });
  await priorEmploymentRegion.getByLabel('Tax year starting').fill(String(taxYearStarting));
  await priorEmploymentGroup.getByRole('radio', { name: 'No', exact: true }).check();
  await priorEmploymentRegion.getByRole('button', { name: 'Save prior employment' }).click();
  await expect(
    page.getByText(`Recorded: no prior employment in tax year ${taxYearStarting}.`),
  ).toBeVisible();

  // Declare UnsupportedDeductionStatus: none, from the same date pay
  // starts. The form requires a reason even for "none".
  const unsupportedDeductionsRegion = page.getByRole('region', {
    name: 'Unsupported deductions',
    exact: true,
  });
  const unsupportedDeductionsGroup = unsupportedDeductionsRegion.getByRole('radiogroup', {
    name: 'Does this employee have any deductions Salt does not support?',
  });
  await unsupportedDeductionsRegion.getByLabel('Effective from').fill(periodStartText);
  await unsupportedDeductionsGroup.getByRole('radio', { name: 'No', exact: true }).check();
  await unsupportedDeductionsRegion.getByLabel('Reason').fill('no unsupported deductions');
  await unsupportedDeductionsRegion
    .getByRole('button', { name: 'Save unsupported deductions' })
    .click();
  await expect(
    page.getByText(`Recorded: no unsupported deductions from ${periodStartText}.`),
  ).toBeVisible();

  // Back to the Employer's own home — reached from here only by the
  // browser's own back button, since no screen this deep links there
  // directly — then on to Payroll.
  await page.goBack();
  await page.goBack();
  await page.getByRole('link', { name: 'Payroll' }).click();

  // Create an Ordinary run over the one pay period this Employer's
  // calendar-month schedule generates for today.
  await page.getByLabel('Period start').fill(periodStartText);
  await page.getByLabel('Period end').fill(periodEndText);
  await page.getByLabel('Pay date').fill(payDateText);
  await page.getByRole('button', { name: 'Create run' }).click();

  // See the member proposed, with no blockers standing in the way of
  // Calculate: both declarations and CompensationTerms are already in
  // force.
  await expect(page.getByRole('heading', { level: 3, name: 'Ada Lovelace' })).toBeVisible();
  await expect(page.getByText('No standing blockers.')).toBeVisible();

  // Add a taxable allowance.
  await page.getByRole('button', { name: 'Add a taxable allowance' }).click();
  await page.getByLabel('Taxable allowance').fill('200.00');
  await page.getByRole('button', { name: 'Save earnings' }).click();
  await expect(page.getByText('Earnings saved.')).toBeVisible();

  // Calculate, then read the nine figures the wire carries (§0.29).
  await page.getByRole('button', { name: 'Calculate' }).click();
  const figures = await readFigures(page);

  // `basicPayCents` is prorated (see above), so this only bounds it: never
  // nothing, and never more than the full R15000.00 recorded.
  expect(figures.basicPayCents).toBeGreaterThan(0);
  expect(figures.basicPayCents).toBeLessThanOrEqual(1_500_000);
  // Proration never touches an allowance (`calculation.rs`'s own
  // `salt_policy_proration_never_touches_an_allowance`), so this one is
  // exact.
  expect(figures.taxableAllowancesCents).toBe(20_000);
  expect(figures.grossCents).toBe(figures.basicPayCents + figures.taxableAllowancesCents);
  expect(figures.taxableRemunerationCents).toBe(figures.grossCents);
  // The calculator really ran: both social security figures are positive.
  expect(figures.employeeSscCents).toBeGreaterThan(0);
  expect(figures.employerSscCents).toBeGreaterThan(0);
  // PAYE is deliberately not asserted positive — see
  // `payroll_journey.rs`'s own comment on the same figure, for the same
  // reason: cumulative PAYE owes nothing yet this early in the tax year.
  expect(figures.payeCents).toBeGreaterThanOrEqual(0);
  expect(figures.totalDeductionsCents).toBe(figures.payeCents + figures.employeeSscCents);
  expect(figures.netCents).toBe(figures.grossCents - figures.totalDeductionsCents);

  // Finalize through the confirmation. The confirmation is the deliberate
  // step (§0.26): it has to be on the screen, and it has to say how many
  // people becoming history covers, before the second click is a decision
  // rather than a ritual.
  await page.getByRole('button', { name: 'Finalize', exact: true }).click();
  await expect(
    page.getByText('This creates immutable payroll history for 1 person.'),
  ).toBeVisible();
  await page.getByRole('button', { name: 'Confirm finalize' }).click();

  // See the finalized payroll: the one member this run finalized navigates
  // straight there, and its figures read exactly as Calculate's did.
  await expect(page.getByText('This payroll is finalized and cannot be changed.')).toBeVisible();
  expect(await readFigures(page)).toEqual(figures);
  const finalizedUrl = page.url();
  // The screen changed because the URL did — a finalized payroll of its
  // own, never the run screen redrawing itself.
  expect(finalizedUrl).toMatch(/\/app\/employers\/[^/]+\/finalized\/[^/]+$/);

  // Reload and see the same finalized payroll. Load-bearing (issue #68's
  // own Deep Instructions): this proves the figures came from the server
  // and not from anything the browser was holding.
  await page.reload();
  expect(page.url()).toBe(finalizedUrl);
  await expect(page.getByText('This payroll is finalized and cannot be changed.')).toBeVisible();
  expect(await readFigures(page)).toEqual(figures);
});
