// `GET /api/employers/{e}/payroll-runs/{r}/payment-summary` (issue #83,
// parent #70 §D-10): names and net pay for a finalized run's live records
// only, and how many rows were excluded as reversed. Same refusals as
// `usePayrollRegister`.

import { useQuery } from '@tanstack/react-query';
import { apiFetch } from '../api/client';
import type { PaymentSummaryResponse } from '../api/types';
import { useEmployerId } from '../employments/useEmployments';

export function paymentSummaryQueryKey(employerId: string, payrollRunId: string) {
  return ['employers', employerId, 'payrollRuns', payrollRunId, 'paymentSummary'] as const;
}

export function usePaymentSummary(payrollRunId: string) {
  const employerId = useEmployerId();

  return useQuery({
    queryKey: paymentSummaryQueryKey(employerId, payrollRunId),
    queryFn: () =>
      apiFetch<PaymentSummaryResponse>(
        `/api/employers/${encodeURIComponent(employerId)}/payroll-runs/${encodeURIComponent(payrollRunId)}/payment-summary`,
      ),
  });
}
