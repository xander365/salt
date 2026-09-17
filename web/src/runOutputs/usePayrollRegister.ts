// `GET /api/employers/{e}/payroll-runs/{r}/register` (issue #83, parent #70
// §D-9): every `FinalizedPayroll` a finalized run produced, with each row's
// liveness and the run's two totals. 409 `payroll_run_not_finalized` for a
// Draft or Calculated run — `PayrollRegister.tsx` reads that off `isError`/
// `error` the same way every other screen reads a refusal, never a special
// case here.

import { useQuery } from '@tanstack/react-query';
import { apiFetch } from '../api/client';
import type { PayrollRegisterResponse } from '../api/types';
import { useEmployerId } from '../employments/useEmployments';

/** Keyed by Employer as well as the run (`usePayrollRunQueryKey`'s own
 * reason): switching Employer must clear this alongside every other
 * Employer-scoped query, never serve a stale register from the last one. */
export function payrollRegisterQueryKey(employerId: string, payrollRunId: string) {
  return ['employers', employerId, 'payrollRuns', payrollRunId, 'register'] as const;
}

export function usePayrollRegister(payrollRunId: string) {
  const employerId = useEmployerId();

  return useQuery({
    queryKey: payrollRegisterQueryKey(employerId, payrollRunId),
    queryFn: () =>
      apiFetch<PayrollRegisterResponse>(
        `/api/employers/${encodeURIComponent(employerId)}/payroll-runs/${encodeURIComponent(payrollRunId)}/register`,
      ),
  });
}
