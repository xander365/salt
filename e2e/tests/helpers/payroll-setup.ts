import { readFile } from 'node:fs/promises';
import { type Page, expect } from '@playwright/test';
import { type BootstrappedOperator, CREDENTIALS_PATH } from '../../global-setup.js';

export interface PayPeriod {
  start: string;
  end: string;
  payDate: string;
  taxYearStarting: number;
}

function isoDate(date: Date): string {
  const year = String(date.getFullYear()).padStart(4, '0');
  const month = String(date.getMonth() + 1).padStart(2, '0');
  const day = String(date.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

export function calendarMonth(offset: number): PayPeriod {
  const today = new Date();
  const start = new Date(today.getFullYear(), today.getMonth() + offset, 1);
  const end = new Date(today.getFullYear(), today.getMonth() + offset + 1, 0);
  const payDate = new Date(today.getFullYear(), today.getMonth() + offset + 1, 5);
  return {
    start: isoDate(start),
    end: isoDate(end),
    payDate: isoDate(payDate),
    taxYearStarting: start.getMonth() + 1 >= 3 ? start.getFullYear() : start.getFullYear() - 1,
  };
}

export async function signIn(page: Page): Promise<BootstrappedOperator> {
  const operator: BootstrappedOperator = JSON.parse(await readFile(CREDENTIALS_PATH, 'utf-8'));
  await page.goto('/login');
  await page.getByLabel('Email').fill(operator.email);
  await page.getByLabel('Password').fill(operator.password);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await expect(page.getByRole('heading', { level: 1, name: operator.employerName })).toBeVisible();
  return operator;
}

export async function ensureEmployerParticulars(page: Page): Promise<void> {
  const registeredName = page.getByLabel('Registered name');
  if ((await registeredName.inputValue()) !== '') return;

  await registeredName.fill('Acme Corp (Pty) Ltd');
  await page.getByLabel('Address line 1').fill('1 Independence Ave');
  await page.getByLabel('City').fill('Windhoek');
  const region = page.getByRole('region', { name: 'Employer particulars' });
  await region.getByRole('button', { name: 'Save particulars' }).click();
  await expect(region.getByText('Particulars saved.')).toBeVisible();
}

export async function addFullyDeclaredEmployment(
  page: Page,
  {
    fullName,
    identityNumber,
    basicPay,
    period,
  }: {
    fullName: string;
    identityNumber: string;
    basicPay: string;
    period: PayPeriod;
  },
): Promise<void> {
  await page.getByLabel('Full name').fill(fullName);
  await page.getByRole('button', { name: 'Add person' }).click();
  await expect(page.getByRole('status')).toHaveText(`Added ${fullName}.`);
  await page.getByRole('link', { name: fullName, exact: true }).click();

  const particulars = page.getByRole('region', { name: 'Identity number and address' });
  await particulars.getByLabel('Identity number').fill(identityNumber);
  await particulars.getByLabel('Address line 1').fill('10 Independence Avenue');
  await particulars.getByLabel('City').fill('Windhoek');
  await particulars.getByRole('button', { name: 'Save particulars' }).click();
  await expect(particulars.getByText('Particulars saved.')).toBeVisible();

  const pay = page.getByRole('region', { name: 'Pay', exact: true });
  await pay.getByLabel('Effective from').fill(period.start);
  await pay.getByLabel('Basic pay').fill(basicPay);
  await pay.getByLabel('Ordinary hours per week').fill('40.00');
  await pay.getByRole('button', { name: 'Save pay' }).click();
  await expect(
    page.getByText(
      `Pay of ${basicPay} for 40.00 ordinary hours per week recorded from ${period.start}.`,
    ),
  ).toBeVisible();

  // This fixture may be created after another spec has already finalized
  // the preceding period. Recording the first period Salt owns makes that
  // history boundary explicit instead of relying on another test's run
  // membership. Zero figures are correct: these fictional Employments have
  // no earlier pay in the tax year.
  const openingBalance = page.getByRole('region', { name: 'Opening balance' });
  await openingBalance.getByLabel('Tax year starting').fill(String(period.taxYearStarting));
  await openingBalance.getByLabel('First Salt pay period (end date)').fill(period.end);
  await openingBalance.getByRole('button', { name: 'Save opening balance' }).click();
  await expect(openingBalance.getByRole('status')).toHaveText(
    `Recorded: Salt starts with the pay period ending ${period.end}; earlier periods carry prior taxable remuneration 0.00 and prior PAYE 0.00.`,
  );

  const priorEmployment = page.getByRole('region', { name: 'Prior employment', exact: true });
  await priorEmployment.getByLabel('Tax year starting').fill(String(period.taxYearStarting));
  await priorEmployment.getByRole('radio', { name: 'No', exact: true }).check();
  await priorEmployment.getByRole('button', { name: 'Save prior employment' }).click();
  await expect(
    priorEmployment.getByText(
      `Recorded: no prior employment in tax year ${period.taxYearStarting}.`,
    ),
  ).toBeVisible();

  const unsupported = page.getByRole('region', { name: 'Unsupported deductions', exact: true });
  await unsupported.getByLabel('Effective from').fill(period.start);
  await unsupported.getByRole('radio', { name: 'No', exact: true }).check();
  await unsupported.getByLabel('Reason').fill('no unsupported deductions');
  await unsupported.getByRole('button', { name: 'Save unsupported deductions' }).click();
  await expect(
    unsupported.getByText(`Recorded: no unsupported deductions from ${period.start}.`),
  ).toBeVisible();
}

export async function endEmployment(page: Page, fullName: string, endDate: string): Promise<void> {
  await page.getByRole('link', { name: fullName, exact: true }).click();
  const leaver = page.getByRole('region', { name: 'Leaver' });
  await leaver.getByLabel('Last day employed').fill(endDate);
  await leaver.getByLabel('Reason').fill('end browser fixture before the next period');
  await leaver.getByRole('button', { name: 'Record end date' }).click();
  await expect(leaver.getByRole('status')).toHaveText(`Recorded: employment ends ${endDate}.`);
}
