import { renderToStaticMarkup } from 'react-dom/server';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, test } from 'vitest';
import type { FiguresDto } from '../api/types';
import { PaymentSummaryResults } from '../routes/PaymentSummary';
import { PayrollRegisterResults } from '../routes/PayrollRegister';

const ZERO_FIGURES: FiguresDto = {
  basicPayCents: 0,
  taxableAllowancesCents: 0,
  overtimeCents: 0,
  grossCents: 0,
  taxableRemunerationCents: 0,
  payeCents: 0,
  employeeSscCents: 0,
  medicalAidPremiumCents: 0,
  employerSscCents: 0,
  totalDeductionsCents: 0,
  netCents: 0,
};

describe('empty run output results', () => {
  test('an all-reversed payment summary keeps its zero total and excluded count visible', () => {
    const markup = renderToStaticMarkup(
      <MemoryRouter>
        <PaymentSummaryResults
          employerId="employer-id"
          rows={[]}
          totalNetPayCents={0}
          excludedReversedCount={2}
        />
      </MemoryRouter>,
    );

    expect(markup).toContain('No live record from this run to pay.');
    expect(markup).toContain('<tbody></tbody>');
    expect(markup).toContain('Total net pay');
    expect(markup).toContain('N$0.00');
    expect(markup).toContain('Excluded as reversed: 2');
  });

  test('an empty payroll register keeps both zero total lines visible', () => {
    const markup = renderToStaticMarkup(
      <MemoryRouter>
        <PayrollRegisterResults
          employerId="employer-id"
          rows={[]}
          totalAsFinalized={ZERO_FIGURES}
          totalStillLive={ZERO_FIGURES}
        />
      </MemoryRouter>,
    );

    expect(markup).toContain('This run finalized nobody');
    expect(markup).toContain('<tbody></tbody>');
    expect(markup).toContain('Total as finalized by this run');
    expect(markup).toContain('Total still live from this run');
    expect(markup).toContain('N$0.00');
  });
});
