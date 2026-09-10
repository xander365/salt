// The ten figures §0.29 names, shared verbatim between a working run's own
// detail/calculate response (`PayrollRun.tsx`) and a finalized payroll's own
// detail response (`FinalizedPayroll.tsx`, issue #66) — the same reason
// `crates/salt-server/src/finalized_payroll.rs` reuses `payroll_runs.rs`'s
// own `FiguresDto` on the server side, so a figure reads identically
// whichever screen shows it.

import { useId } from 'react';
import type { FiguresDto } from '../../api/types';
import { moneyDisplayText } from '../../money';

/** The ten figures §0.29 names, in the order it names them. */
const FIGURE_FIELDS: { key: keyof FiguresDto; label: string }[] = [
  { key: 'basicPayCents', label: 'Basic Pay' },
  { key: 'taxableAllowancesCents', label: 'Taxable Allowances' },
  { key: 'overtimeCents', label: 'Overtime' },
  { key: 'grossCents', label: 'Gross' },
  { key: 'taxableRemunerationCents', label: 'Taxable Remuneration' },
  { key: 'payeCents', label: 'PAYE' },
  { key: 'employeeSscCents', label: 'Employee SSC' },
  { key: 'employerSscCents', label: 'Employer SSC' },
  { key: 'totalDeductionsCents', label: 'Total Deductions' },
  { key: 'netCents', label: 'Net' },
];

export function Figures({ figures }: { figures: FiguresDto }) {
  // One prefix per rendered `Figures`, so two of them on one screen — a run
  // with two calculated members — still give every figure a term of its own
  // to point at.
  const termIdPrefix = useId();

  return (
    <dl className="grid grid-cols-2 gap-x-6 gap-y-2 sm:grid-cols-3">
      {FIGURE_FIELDS.map(({ key, label }) => {
        const termId = `${termIdPrefix}-${key}`;
        return (
          // The wrapping `group` is what carries the accessible name, and it
          // has to: ARIA 1.2 prohibits naming a `term` or a `definition`, so
          // a `dd` can never announce which figure it is on its own. Naming
          // the pair instead is what lets a person — or the browser journey
          // test standing in for one — reach "PAYE" by the word beside it
          // rather than by counting rows.
          <div key={key} role="group" aria-labelledby={termId} className="flex flex-col">
            <dt id={termId} className="text-xs text-muted-foreground">
              {label}
            </dt>
            <dd className="money text-sm font-medium">{moneyDisplayText(figures[key])}</dd>
          </div>
        );
      })}
    </dl>
  );
}
