// The "Outputs" section on a Finalized run screen (issue #83, parent #70
// §D-9, §D-10): every batch output a finalized run produces — every
// payslip as one PDF, the payroll register, and the payment summary. Shown
// only once `PayrollRun.tsx` has read `status === 'finalized'` off its own
// `GET` (§0's Deep Instructions: the browser never decides readiness on its
// own) — there is no gate of this component's own to duplicate that check.

import { FileText } from 'lucide-react';
import { Link } from 'react-router-dom';
import { paymentSummaryPath, payrollRegisterPath } from '../routes/paths';
import { RunPayslipsDownload } from './RunPayslipsDownload';

export function RunOutputs({
  employerId,
  payrollRunId,
}: {
  employerId: string;
  payrollRunId: string;
}) {
  return (
    <section
      aria-labelledby="run-outputs-heading"
      className="flex flex-col gap-4 rounded-xl border bg-card p-6 text-card-foreground shadow-sm"
    >
      <h3 id="run-outputs-heading" className="text-lg font-semibold">
        Outputs
      </h3>

      <RunPayslipsDownload employerId={employerId} payrollRunId={payrollRunId} />

      <ul className="flex flex-col gap-2 text-sm">
        <li>
          <Link
            to={payrollRegisterPath(employerId, payrollRunId)}
            className="flex items-center gap-1.5 font-medium text-primary underline underline-offset-2"
          >
            <FileText className="size-4 shrink-0" aria-hidden="true" />
            Payroll register
          </Link>
        </li>
        <li>
          <Link
            to={paymentSummaryPath(employerId, payrollRunId)}
            className="flex items-center gap-1.5 font-medium text-primary underline underline-offset-2"
          >
            <FileText className="size-4 shrink-0" aria-hidden="true" />
            Payment summary
          </Link>
        </li>
      </ul>
    </section>
  );
}
