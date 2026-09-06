// `GET /api/employers/{e}/finalized-payroll/{f}/traces` (issue #67, parent
// #59 Spec 3 of 3, §0.29). Deliberately a separate call from
// `useFinalizedPayroll`, gated by `enabled`, so opening a finalized payroll
// never fetches the workings — only opening the disclosure does.

import { useQuery } from '@tanstack/react-query';
import { apiFetch } from '../api/client';
import type { FinalizedPayrollTracesResponse } from '../api/types';
import { useEmployerId } from '../employments/useEmployments';

export function finalizedPayrollTracesQueryKey(employerId: string, finalizedPayrollId: string) {
  return ['employers', employerId, 'finalizedPayroll', finalizedPayrollId, 'traces'] as const;
}

export function useFinalizedPayrollTraces(finalizedPayrollId: string, enabled: boolean) {
  const employerId = useEmployerId();

  return useQuery({
    queryKey: finalizedPayrollTracesQueryKey(employerId, finalizedPayrollId),
    queryFn: () =>
      apiFetch<FinalizedPayrollTracesResponse>(
        `/api/employers/${encodeURIComponent(employerId)}/finalized-payroll/${encodeURIComponent(finalizedPayrollId)}/traces`,
      ),
    enabled,
  });
}
