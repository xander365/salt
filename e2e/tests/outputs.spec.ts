// Reversing a finalized payroll is not reachable in the browser until Slice
// 8. The HTTP seam tests cover reversed and replacement rows; this spec extends
// the ordinary browser journey with the next month's two-member run.

import { readFile } from 'node:fs/promises';
import { type Locator, type Page, expect, test } from '@playwright/test';
import { type BootstrappedOperator, CREDENTIALS_PATH } from '../global-setup.js';

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
  const net = page.getByRole('group', { name: 'Net', exact: true });
  await expect(net).toBeVisible();
  return parseCents(await net.getByRole('definition').innerText());
}

test('a finalized run renders and reconciles every output', async ({ page }) => {
  const operator: BootstrappedOperator = JSON.parse(await readFile(CREDENTIALS_PATH, 'utf-8'));

  await page.goto('/login');
  await page.getByLabel('Email').fill(operator.email);
  await page.getByLabel('Password').fill(operator.password);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { level: 1, name: operator.employerName })).toBeVisible();

  // The dependency finalized this two-member run through the UI.
  await page.getByRole('link', { name: 'Payroll' }).click();
  await page.getByRole('link', { name: /, paid / }).click();
  await expect(page.getByRole('heading', { level: 3, name: 'Outputs' })).toBeVisible();

  const adaNet = await finalizedNetPay(page, 'Ada Lovelace');
  await page.goBack();
  const bobNet = await finalizedNetPay(page, 'Bob Marley');
  await page.goBack();

  await expectPdfDownload(page, page.getByRole('button', { name: 'Download all payslips (PDF)' }));
  await page.getByRole('link', { name: 'Payroll register' }).click();
  await expect(page.getByRole('link', { name: 'Ada Lovelace', exact: true })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Bob Marley', exact: true })).toBeVisible();
  await expect(page.getByRole('row', { name: /Total as finalized by this run/ })).toBeVisible();
  const liveRegisterRow = page.getByRole('row', { name: /Total still live from this run/ });
  await expect(liveRegisterRow).toBeVisible();
  const registerNet = parseCents(await liveRegisterRow.getByRole('cell').nth(8).innerText());
  await expectPdfDownload(page, page.getByRole('button', { name: 'Download PDF' }));

  await page.getByRole('link', { name: 'Back to payroll run' }).click();
  await page.getByRole('link', { name: 'Payment summary' }).click();
  await expect(page.getByText('does not mean anyone has been paid', { exact: false })).toBeVisible();
  await expect(page.getByText('Excluded as reversed: 0')).toBeVisible();
  const summaryTotal = parseCents(
    await page
      .getByRole('row', { name: /Total net pay/ })
      .getByRole('cell')
      .innerText(),
  );
  await expectPdfDownload(page, page.getByRole('button', { name: 'Download PDF' }));

  expect(registerNet).toBe(summaryTotal);
  expect(summaryTotal).toBe(adaNet + bobNet);
});
