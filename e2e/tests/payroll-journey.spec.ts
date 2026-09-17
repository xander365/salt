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
import { type Locator, type Page, expect, test } from '@playwright/test';
import { type BootstrappedOperator, CREDENTIALS_PATH } from '../global-setup.js';

/** The ten figures §0.29 names, plus issue #78's medical aid premium, and
 * the accessible names the screen gives them. Order is immaterial: each value is read by what a person calls it,
 * never by where its definition happens to sit in the document. */
const FIGURE_FIELDS = [
  { field: 'basicPayCents', name: 'Basic Pay' },
  { field: 'taxableAllowancesCents', name: 'Taxable Allowances' },
  { field: 'overtimeCents', name: 'Overtime' },
  { field: 'grossCents', name: 'Gross' },
  { field: 'taxableRemunerationCents', name: 'Taxable Remuneration' },
  { field: 'payeCents', name: 'PAYE' },
  { field: 'employeeSscCents', name: 'Employee SSC' },
  { field: 'medicalAidPremiumCents', name: 'Medical Aid Premium' },
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

/** The inverse of `web/src/money.ts`'s own `formatMoneyDisplay` (issue #88:
 * every figure now renders as `N$1,234.56`, right-aligned in tabular
 * numerals) — strips the currency prefix and thousands separators before
 * parsing, in integer arithmetic only, for the same reason the source it
 * mirrors never lets a `Money` value near a float. */
function parseCentsText(text: string): number {
  const match = /^N\$(\d{1,3}(?:,\d{3})*|\d+)\.(\d{2})$/.exec(text.trim());
  if (match === null) {
    throw new Error(`not a cents-exact amount: "${text}"`);
  }
  const [, whole, fraction] = match;
  return Number(BigInt(whole.replace(/,/g, '')) * 100n + BigInt(fraction));
}

/** Reads every figure in `FIGURE_FIELDS` off whichever screen is showing them
 * — a payroll run's own calculated member, or a finalized payroll — both of
 * which share this one `Figures` component. Each named group is its own wait:
 * counting every `definition` on the page would accidentally include facts
 * outside the figures component as the finalized screen grows. */
async function readFigures(page: Page | Locator): Promise<Figures> {
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

  // Record EmployerParticulars (issue #71): the registered name and address
  // a Payslip must freeze and print (issue #82). Without this, the Payslip
  // downloaded below would be refused, naming exactly this as missing.
  const employerParticularsRegion = page.getByRole('region', { name: 'Employer particulars' });
  await employerParticularsRegion.getByLabel('Registered name').fill('Acme Corp (Pty) Ltd');
  await employerParticularsRegion.getByLabel('Address line 1').fill('1 Independence Ave');
  await employerParticularsRegion.getByLabel('City').fill('Windhoek');
  await employerParticularsRegion.getByRole('button', { name: 'Save particulars' }).click();
  await expect(employerParticularsRegion.getByText('Particulars saved.')).toBeVisible();

  // Add a person by full name.
  await page.getByRole('link', { name: 'People' }).click();
  await page.getByLabel('Full name').fill('Ada Lovelace');
  await page.getByRole('button', { name: 'Add person' }).click();
  await expect(page.getByRole('status')).toHaveText('Added Ada Lovelace.');
  await page.getByRole('link', { name: 'Ada Lovelace' }).click();
  await expect(page.getByRole('heading', { level: 2, name: 'Ada Lovelace' })).toBeVisible();

  // Record PersonParticulars (issue #71): the identity number and address a
  // Payslip must freeze and print (issue #82) — the same reason
  // EmployerParticulars was just recorded above.
  const personParticularsRegion = page.getByRole('region', {
    name: 'Identity number and address',
  });
  await personParticularsRegion.getByLabel('Identity number').fill('80012345678');
  await personParticularsRegion.getByLabel('Address line 1').fill('2 Fidel Castro St');
  await personParticularsRegion.getByLabel('City').fill('Swakopmund');
  await personParticularsRegion.getByRole('button', { name: 'Save particulars' }).click();
  await expect(personParticularsRegion.getByText('Particulars saved.')).toBeVisible();

  // Record CompensationTerms, effective from the start of the period this
  // Employment can first be paid for.
  const payRegion = page.getByRole('region', { name: 'Pay', exact: true });
  await payRegion.getByLabel('Effective from').fill(periodStartText);
  await payRegion.getByLabel('Basic pay').fill('15000.00');
  const ordinaryHours = payRegion.getByLabel('Ordinary hours per week');
  for (const invalid of ['0', '-0.01', '168.01', '40.001']) {
    await ordinaryHours.fill(invalid);
    await payRegion.getByRole('button', { name: 'Save pay' }).click();
    await expect(ordinaryHours).toHaveAttribute('aria-invalid', 'true');
    await expect(
      payRegion.getByText(
        'Enter more than 0 and no more than 168 hours per week, with up to two decimal places.',
      ),
    ).toBeVisible();
  }
  await ordinaryHours.fill('40.00');
  await payRegion.getByRole('button', { name: 'Save pay' }).click();
  await expect(
    page.getByText(
      `Pay of 15000.00 for 40.00 ordinary hours per week recorded from ${periodStartText}.`,
    ),
  ).toBeVisible();

  // A real document reload proves the value came back from PostgreSQL via
  // the Employment detail route, not from the form's local React state.
  await page.reload();
  await expect(page.getByRole('heading', { level: 2, name: 'Ada Lovelace' })).toBeVisible();
  await expect(page.getByText('40.00', { exact: true })).toBeVisible();

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

  // A second fully declared Employment makes the finalized run exercise the
  // same multi-person path its outputs use. Ada remains the member whose
  // richer earnings and workings this journey examines below.
  await page.getByRole('link', { name: 'People', exact: true }).click();
  await page.getByLabel('Full name').fill('Bob Marley');
  await page.getByRole('button', { name: 'Add person' }).click();
  await expect(page.getByRole('status')).toHaveText('Added Bob Marley.');
  await page.getByRole('link', { name: 'Bob Marley', exact: true }).click();

  const bobParticulars = page.getByRole('region', { name: 'Identity number and address' });
  await bobParticulars.getByLabel('Identity number').fill('81012345678');
  await bobParticulars.getByLabel('Address line 1').fill('3 Sam Nujoma Drive');
  await bobParticulars.getByLabel('City').fill('Walvis Bay');
  await bobParticulars.getByRole('button', { name: 'Save particulars' }).click();
  await expect(bobParticulars.getByText('Particulars saved.')).toBeVisible();

  const bobPay = page.getByRole('region', { name: 'Pay', exact: true });
  await bobPay.getByLabel('Effective from').fill(periodStartText);
  await bobPay.getByLabel('Basic pay').fill('12000.00');
  await bobPay.getByLabel('Ordinary hours per week').fill('40.00');
  await bobPay.getByRole('button', { name: 'Save pay' }).click();

  const bobPriorEmployment = page.getByRole('region', { name: 'Prior employment', exact: true });
  await bobPriorEmployment.getByLabel('Tax year starting').fill(String(taxYearStarting));
  await bobPriorEmployment.getByRole('radio', { name: 'No', exact: true }).check();
  await bobPriorEmployment.getByRole('button', { name: 'Save prior employment' }).click();

  const bobUnsupportedDeductions = page.getByRole('region', {
    name: 'Unsupported deductions',
    exact: true,
  });
  await bobUnsupportedDeductions.getByLabel('Effective from').fill(periodStartText);
  await bobUnsupportedDeductions.getByRole('radio', { name: 'No', exact: true }).check();
  await bobUnsupportedDeductions.getByLabel('Reason').fill('no unsupported deductions');
  await bobUnsupportedDeductions
    .getByRole('button', { name: 'Save unsupported deductions' })
    .click();

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

  const adaMember = page
    .getByRole('listitem')
    .filter({ has: page.getByRole('heading', { level: 3, name: 'Ada Lovelace' }) });
  // See Ada's proposed member, with no blockers standing in the way of
  // Calculate: both declarations and CompensationTerms are already in
  // force.
  await expect(adaMember.getByRole('heading', { level: 3, name: 'Ada Lovelace' })).toBeVisible();
  await expect(adaMember.getByText('No standing blockers.')).toBeVisible();
  const runUrl = page.url();

  // Add a taxable allowance.
  await adaMember.getByRole('button', { name: 'Add a taxable allowance' }).click();
  await adaMember.getByLabel('Allowance label').fill('standby allowance');
  await adaMember.getByLabel('Amount', { exact: true }).fill('200.00');

  // Add overtime as **hours at a multiplier** (issue #76, D14): the form
  // takes no amount at all, because Salt prices the line itself. The
  // multiplier is a closed set of two (D31), so it is a `<select>` and
  // 1.5 is what a new line starts on.
  await adaMember.getByRole('button', { name: 'Add overtime' }).click();
  await adaMember.getByLabel('Overtime label (optional)').fill('Sunday overtime');
  await adaMember.getByLabel('Hours', { exact: true }).fill('10');
  await expect(adaMember.getByLabel('Multiplier')).toHaveValue('1.5');

  // Withhold the employee's own medical aid premium (issue #78): a
  // voluntary deduction, taken after PAYE and social security at exactly the
  // amount entered.
  await adaMember.getByRole('button', { name: 'Add a medical aid premium' }).click();
  await adaMember.getByLabel('Premium amount').fill('500.00');
  await adaMember.getByRole('button', { name: 'Save earnings' }).click();
  await expect(page.getByText('Earnings saved.')).toBeVisible();
  await page.getByRole('button', { name: 'Save earnings' }).last().click();

  // Calculate, then read the figures the wire carries (§0.29, issue #78).
  await page.getByRole('button', { name: 'Calculate' }).click();
  const figures = await readFigures(adaMember);

  // `basicPayCents` is prorated (see above), so this only bounds it: never
  // nothing, and never more than the full R15000.00 recorded.
  expect(figures.basicPayCents).toBeGreaterThan(0);
  expect(figures.basicPayCents).toBeLessThanOrEqual(1_500_000);
  // Proration never touches an allowance (`calculation.rs`'s own
  // `salt_policy_proration_never_touches_an_allowance`), so this one is
  // exact.
  expect(figures.taxableAllowancesCents).toBe(20_000);
  // Overtime is exact too, and for a sharper reason: the derived hourly
  // rate comes from the **contractual** N$15,000.00 on the terms row and
  // never from the prorated Basic Pay above (SC-OPEN-6, ADR-0022) — a
  // person's hourly rate does not fall because they joined mid-month.
  //   rate  = 1,500,000c x 12 / 52 / 40 = 1125 / 13 N$ per hour, exactly
  //   line  = 1125 / 13 x 10 hours x 1.5 = 1,298.0769... -> N$1,298.08
  // Rounded once, at the line. This figure therefore does not move with
  // proration, and asserting it exactly is the whole point.
  expect(figures.overtimeCents).toBe(129_808);
  expect(figures.grossCents).toBe(
    figures.basicPayCents + figures.taxableAllowancesCents + figures.overtimeCents,
  );
  expect(figures.taxableRemunerationCents).toBe(figures.grossCents);
  // The calculator really ran: both social security figures are positive.
  expect(figures.employeeSscCents).toBeGreaterThan(0);
  expect(figures.employerSscCents).toBeGreaterThan(0);
  // PAYE is deliberately not asserted positive — see
  // `payroll_journey.rs`'s own comment on the same figure, for the same
  // reason: cumulative PAYE owes nothing yet this early in the tax year.
  expect(figures.payeCents).toBeGreaterThanOrEqual(0);
  // The premium is its own figure, at exactly the amount entered, and the
  // only thing between the statutory deductions and net pay (SC-OPEN-7).
  expect(figures.medicalAidPremiumCents).toBe(50_000);
  expect(figures.totalDeductionsCents).toBe(
    figures.payeCents + figures.employeeSscCents + figures.medicalAidPremiumCents,
  );
  expect(figures.netCents).toBe(figures.grossCents - figures.totalDeductionsCents);

  // Saving an unchanged earnings list is not a change and must not describe
  // the figures as stale.
  await Promise.all([
    page.waitForResponse(
      (response) => response.request().method() === 'PUT' && response.url().endsWith('/pay-lines'),
    ),
    adaMember.getByRole('button', { name: 'Save earnings' }).click(),
  ]);
  await expect(page.getByText('Earnings changed since these figures were calculated.')).toHaveCount(
    0,
  );

  // Change the line shape without changing its total. This specifically
  // proves Calculate clears the stale state from its own success rather
  // than relying on an equal Figures object receiving a new identity.
  await adaMember.getByRole('textbox', { name: 'Amount', exact: true }).fill('100.00');
  await adaMember.getByRole('button', { name: 'Add a taxable allowance' }).click();
  await adaMember
    .getByRole('textbox', { name: 'Allowance label', exact: true })
    .nth(1)
    .fill('travel');
  await adaMember.getByRole('textbox', { name: 'Amount', exact: true }).nth(1).fill('100.00');
  await adaMember.getByRole('button', { name: 'Save earnings' }).click();
  await expect(
    page.getByText('Earnings changed since these figures were calculated.'),
  ).toBeVisible();
  // The warning *replaces* the figures rather than sitting above them
  // (§0's story 75: "say so and hide the old figures") — every one of the
  // figures predates the edit that was just saved, so none of them is on the
  // screen to be read.
  await expect(adaMember.getByRole('definition')).toHaveCount(0);
  await page.getByRole('button', { name: 'Calculate' }).click();
  await expect(page.getByText('Earnings changed since these figures were calculated.')).toHaveCount(
    0,
  );
  expect(await readFigures(adaMember)).toEqual(figures);

  // Finalize through the confirmation. The confirmation is the deliberate
  // step (§0.26): it has to be on the screen, and it has to say how many
  // people becoming history covers, before the second click is a decision
  // rather than a ritual.
  await page.getByRole('button', { name: 'Finalize', exact: true }).click();
  await expect(
    page.getByText('This creates immutable payroll history for 2 people.'),
  ).toBeVisible();
  await page.getByRole('button', { name: 'Confirm finalize' }).click();

  // See Ada's finalized payroll. A two-member run stays on the run screen and
  // offers each finalized record explicitly, rather than guessing one.
  await page.getByRole('link', { name: 'Ada Lovelace', exact: true }).click();
  await expect(page.getByText('This payroll is finalized and cannot be changed.')).toBeVisible();
  expect(await readFigures(page)).toEqual(figures);
  const finalizedUrl = page.url();

  // Open the workings and check the overtime line by hand, which is the
  // whole point of issue #76's acceptance criteria. Every figure behind
  // N$1,298.08 has to be on the screen: the contractual Basic Pay, the
  // ordinary hours, both halves of the divisor, the derived rate, the
  // hours and the multiplier.
  await page.getByText('How this pay was worked out').click();
  const overtimeWorkings = page.getByRole('region', { name: 'Sunday overtime workings' });
  await expect(overtimeWorkings.getByText('N$15,000.00')).toBeVisible();
  await expect(overtimeWorkings.getByText('40.00', { exact: true })).toBeVisible();
  await expect(overtimeWorkings.getByText('× 12 ÷ 52 ÷ 40.00')).toBeVisible();
  // The rate repeats forever, so it is shown as an approximation *and* as
  // the exact fraction Salt actually multiplied. Neither alone is honest.
  await expect(overtimeWorkings.getByText('≈ N$86.5385')).toBeVisible();
  await expect(overtimeWorkings.getByText('exactly 1125 ÷ 13')).toBeVisible();
  await expect(overtimeWorkings.getByText('10', { exact: true })).toBeVisible();
  await expect(overtimeWorkings.getByText('× 1.5')).toBeVisible();
  await expect(overtimeWorkings.getByText('N$1,298.08')).toBeVisible();

  // And it must say plainly that the divisor is Salt's own unconfirmed
  // choice. This sentence is the acceptance criterion that matters most:
  // the screen may never let an Operator read the divisor as Namibian law.
  await expect(
    overtimeWorkings.getByText(
      "This divisor is Salt's own policy (SC-OPEN-6), not Namibian law. It is awaiting confirmation.",
    ),
  ).toBeVisible();
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

  // Download the Payslip (issue #82 review follow-up): a real browser
  // download, off the real network, never a mocked response. The finalized
  // payroll's own id is the last path segment of `finalizedUrl`.
  const finalizedPayrollId = finalizedUrl.split('/').pop();
  await expectPayslipDownload(
    page,
    page.getByRole('button', { name: 'Download payslip', exact: true }),
    `payslip-${finalizedPayrollId}.pdf`,
  );

  // And from the finalized run screen itself (issue #82's first acceptance
  // criterion): the run lists each finalized person with their own
  // download.
  await page.goto(runUrl);
  await expectPayslipDownload(
    page,
    page.getByRole('button', { name: 'Download payslip for Ada Lovelace' }),
    `payslip-${finalizedPayrollId}.pdf`,
  );
});

/** Clicks `button`, waits for the real browser download it starts, and
 * checks the saved file's name and that it is a PDF. */
async function expectPayslipDownload(page: Page, button: Locator, filename: string) {
  const [download] = await Promise.all([page.waitForEvent('download'), button.click()]);
  expect(download.suggestedFilename()).toBe(filename);
  const stream = await download.createReadStream();
  const chunks: Buffer[] = [];
  for await (const chunk of stream) {
    chunks.push(chunk as Buffer);
  }
  expect(Buffer.concat(chunks).subarray(0, 4).toString('ascii')).toBe('%PDF');
}
