// The nine figures §0.29 names, shared verbatim between a working run's own
// detail/calculate response (`PayrollRun.tsx`) and a finalized payroll's own
// detail response (`FinalizedPayroll.tsx`, issue #66) — the same reason
// `crates/salt-server/src/finalized_payroll.rs` reuses `payroll_runs.rs`'s
// own `FiguresDto` on the server side, so a figure reads identically
// whichever screen shows it.

import type { FiguresDto } from '../../api/types';
import { formatCents } from '../../money';

/**
 * `formatCents` throws rather than show an amount it cannot render exactly
 * (INV-001, `money.ts`). Thrown from here that would blank the whole screen
 * over one figure, hiding every other one — so the one figure says what it
 * cannot show and the rest still reads.
 */
function centsText(cents: number): string {
  try {
    return formatCents(cents);
  } catch {
    return 'an amount that cannot be displayed exactly';
  }
}

/** The nine figures §0.29 names, in the order it names them. */
const FIGURE_FIELDS: { key: keyof FiguresDto; label: string }[] = [
  { key: 'basicPayCents', label: 'Basic Pay' },
  { key: 'taxableAllowancesCents', label: 'Taxable Allowances' },
  { key: 'grossCents', label: 'Gross' },
  { key: 'taxableRemunerationCents', label: 'Taxable Remuneration' },
  { key: 'payeCents', label: 'PAYE' },
  { key: 'employeeSscCents', label: 'Employee SSC' },
  { key: 'employerSscCents', label: 'Employer SSC' },
  { key: 'totalDeductionsCents', label: 'Total Deductions' },
  { key: 'netCents', label: 'Net' },
];

export function Figures({ figures }: { figures: FiguresDto }) {
  return (
    <dl>
      {FIGURE_FIELDS.map(({ key, label }) => (
        <div key={key}>
          <dt>{label}</dt>
          <dd>{centsText(figures[key])}</dd>
        </div>
      ))}
    </dl>
  );
}
