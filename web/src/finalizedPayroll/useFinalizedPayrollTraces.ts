// `GET /api/employers/{e}/finalized-payroll/{f}/traces` (issue #67, parent
// #59 Spec 3 of 3, §0.29). Deliberately a separate call from
// `useFinalizedPayroll`, gated by `enabled`, so opening a finalized payroll
// never fetches the workings — only opening the disclosure does.
//
// A `FinalizedPayroll` never changes once written, and neither do its
// traces, so `staleTime: Infinity` is a statement of fact and not a tuning
// choice: closing and reopening the disclosure, or refocusing the window,
// must not spend a request re-reading an answer that cannot have moved.

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
    staleTime: Infinity,
  });
}
