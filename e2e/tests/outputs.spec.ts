// Reversing a finalized payroll is not reachable in the browser until Slice
// 8. The HTTP seam tests cover reversed and replacement rows. This focused
// spec creates and finalizes its own two-Employment run entirely through the
// UI before it examines any output.

import { type Locator, type Page, expect, test } from '@playwright/test';
import {
  addFullyDeclaredEmployment,
  calendarMonth,
  endEmployment,
  ensureEmployerParticulars,
  signIn,
} from './helpers/payroll-setup.js';

function parseCents(text: string): number {
  const match = /^N\$(\d{1,3}(?:,\d{3})*|\d+)\.(\d{2})$/.exec(text.trim());
  if (match === null) throw new Error(`expected a money value, got ${text}`);
  return Number(BigInt(match[1].replace(/,/g, '')) * 100n + BigInt(match[2]));
}

async function expectPdfDownload(page: Page, trigger: Locator) {
  const [download] = await Promise.all([page.waitForEvent('download'), trigger.click()]);
  expect(download.suggestedFilename()).toMatch(/\.pdf$/);
  const stream = await download.createReadStream();
  const chunks: Buffer[] = [];
  for await (const chunk of stream!) chunks.push(chunk as Buffer);
  const bytes = Buffer.concat(chunks);
  expect(bytes.subarray(0, 5).toString('ascii')).toBe('%PDF-');
  expect(bytes.length).toBeGreaterThan(1_000);
}

async function finalizedNetPay(page: Page, name: string): Promise<number> {
  await page.getByRole('link', { name, exact: true }).click();
  // The run screen also shows a Net figure per member; wait until the
  // single payroll's own screen has replaced it.
  await expect(
    page.getByRole('heading', { level: 2, name: `${name}:`, exact: false }),
  ).toBeVisible();
  const net = page.getByRole('group', { name: 'Net', exact: true });
  await expect(net).toBeVisible();
  return parseCents(await net.getByRole('definition').innerText());
}

test('a finalized run renders and reconciles every output', async ({ page }) => {
  await signIn(page);
  await ensureEmployerParticulars(page);

  // When the original journey has already run, use the following calendar
  // month and retire its fixture first. When this spec runs alone, use the
  // current month. Either way this spec itself creates exactly two eligible
  // Employments and never consumes another test's finalized run. Decide
  // from the run list the page itself loads: counting links right after the
  // click races that load and would always pick the current month.
  const runList = page.waitForResponse(
    (response) => response.request().method() === 'GET' && /\/payroll-runs$/.test(response.url()),
  );
  await page.getByRole('link', { name: 'Payroll' }).click();
  const { payrollRuns } = (await (await runList).json()) as { payrollRuns: unknown[] };
  const followsExistingRun = payrollRuns.length > 0;
  const period = calendarMonth(followsExistingRun ? 1 : 0);

  await page.getByRole('link', { name: 'People' }).click();
  if (followsExistingRun) {
    await endEmployment(page, 'Ada Lovelace', calendarMonth(0).end);
    await page.getByRole('link', { name: 'People', exact: true }).click();
  }

  await addFullyDeclaredEmployment(page, {
    fullName: 'Katherine Johnson',
    identityNumber: '18012345678',
    basicPay: '16000.00',
    period,
  });
  await page.getByRole('link', { name: 'People', exact: true }).click();
  await addFullyDeclaredEmployment(page, {
    fullName: 'Alan Turing',
    identityNumber: '19012345678',
    basicPay: '14000.00',
    period,
  });

  await page.getByRole('link', { name: 'Payroll' }).click();
  await page.getByLabel('Period start').fill(period.start);
  await page.getByLabel('Period end').fill(period.end);
  await page.getByLabel('Pay date').fill(period.payDate);
  await page.getByRole('button', { name: 'Create run' }).click();

  await expect(page.getByRole('heading', { level: 3, name: 'Katherine Johnson' })).toBeVisible();
  await expect(page.getByRole('heading', { level: 3, name: 'Alan Turing' })).toBeVisible();
  await page.getByRole('button', { name: 'Calculate' }).click();
  await page.getByRole('button', { name: 'Finalize', exact: true }).click();
  await expect(
    page.getByText('This creates immutable payroll history for 2 people.'),
  ).toBeVisible();
  await page.getByRole('button', { name: 'Confirm finalize' }).click();
  await expect(page.getByRole('heading', { level: 3, name: 'Outputs' })).toBeVisible();

  const katherineNet = await finalizedNetPay(page, 'Katherine Johnson');
  await page.goBack();
  const alanNet = await finalizedNetPay(page, 'Alan Turing');
  await page.goBack();

  await expectPdfDownload(page, page.getByRole('button', { name: 'Download all payslips (PDF)' }));
  await page.getByRole('link', { name: 'Payroll register' }).click();
  await expect(page.getByRole('link', { name: 'Katherine Johnson', exact: true })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Alan Turing', exact: true })).toBeVisible();
  await expect(page.getByRole('row', { name: /Total as finalized by this run/ })).toBeVisible();
  const liveRegisterRow = page.getByRole('row', { name: /Total still live from this run/ });
  await expect(liveRegisterRow).toBeVisible();
  const registerNet = parseCents(await liveRegisterRow.getByRole('cell').nth(8).innerText());
  await expectPdfDownload(page, page.getByRole('button', { name: 'Download PDF' }));

  await page.getByRole('link', { name: 'Back to payroll run' }).click();
  await page.getByRole('link', { name: 'Payment summary' }).click();
  await expect(
    page.getByText('does not mean anyone has been paid', { exact: false }),
  ).toBeVisible();
  await expect(page.getByText('Excluded as reversed: 0')).toBeVisible();
  const summaryTotal = parseCents(
    await page
      .getByRole('row', { name: /Total net pay/ })
      .getByRole('cell')
      .innerText(),
  );
  await expectPdfDownload(page, page.getByRole('button', { name: 'Download PDF' }));

  expect(registerNet).toBe(summaryTotal);
  expect(summaryTotal).toBe(katherineNet + alanNet);
});
