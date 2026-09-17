// The three run-level PDFs issue #83 adds: `GET .../payroll-runs/{r}/
// payslips.pdf`, `.../register.pdf` and `.../payment-summary.pdf`. Modelled
// as a mutation, not a query, for the same reason `usePayslipDownload`
// (issue #82) is: an Operator asks for this on demand, the response is a
// file rather than cacheable JSON, and `salt-server` renders it fresh —
// `Cache-Control: no-store` — on every request, so there is nothing here
// for React Query to ever treat as stale.

import { useMutation } from '@tanstack/react-query';
import { apiDownload } from '../api/client';
import { useEmployerId } from '../employments/useEmployments';

export type RunPdfKind = 'payslips' | 'register' | 'paymentSummary';

function runPdfFilenameSuffix(kind: RunPdfKind): string {
  switch (kind) {
    case 'payslips':
      return 'payslips.pdf';
    case 'register':
      return 'register.pdf';
    case 'paymentSummary':
      return 'payment-summary.pdf';
  }
}

export function useRunPdfDownload(payrollRunId: string, kind: RunPdfKind) {
  const employerId = useEmployerId();

  return useMutation({
    mutationFn: () =>
      apiDownload(
        `/api/employers/${encodeURIComponent(employerId)}/payroll-runs/${encodeURIComponent(payrollRunId)}/${runPdfFilenameSuffix(kind)}`,
      ),
  });
}
