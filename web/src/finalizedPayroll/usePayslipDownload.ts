// `GET /api/employers/{e}/finalized-payroll/{f}/payslip.pdf` (issue #82,
// review follow-up). Modelled as a mutation, not a query: an Operator asks
// for this on demand by clicking a button, the response is a file rather
// than cacheable JSON, and `salt-server` renders it fresh — and marks it
// `Cache-Control: no-store` — on every single request, so there is nothing
// here for React Query to ever treat as stale and refetch on its own.

import { useMutation } from '@tanstack/react-query';
import { apiDownload } from '../api/client';
import { useEmployerId } from '../employments/useEmployments';

export function usePayslipDownload(finalizedPayrollId: string) {
  const employerId = useEmployerId();

  return useMutation({
    mutationFn: () =>
      apiDownload(
        `/api/employers/${encodeURIComponent(employerId)}/finalized-payroll/${encodeURIComponent(finalizedPayrollId)}/payslip.pdf`,
      ),
  });
}
