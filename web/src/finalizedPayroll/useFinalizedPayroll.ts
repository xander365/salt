// `GET /api/employers/{e}/finalized-payroll/{f}` (issue #66, parent #59 Spec
// 3 of 3, §0.22/§0.29). One immutable finalized payroll: the ten figures,
// the period, the pay date and the SaltVersion. The same answer every time
// it is read, because nothing about a `FinalizedPayroll` ever changes.

import { useQuery } from '@tanstack/react-query';
import { apiFetch } from '../api/client';
import type { FinalizedPayrollDetailResponse } from '../api/types';
import { useEmployerId } from '../employments/useEmployments';

export function finalizedPayrollQueryKey(employerId: string, finalizedPayrollId: string) {
  return ['employers', employerId, 'finalizedPayroll', finalizedPayrollId] as const;
}

export function useFinalizedPayroll(finalizedPayrollId: string) {
  const employerId = useEmployerId();

  return useQuery({
    queryKey: finalizedPayrollQueryKey(employerId, finalizedPayrollId),
    queryFn: () =>
      apiFetch<FinalizedPayrollDetailResponse>(
        `/api/employers/${encodeURIComponent(employerId)}/finalized-payroll/${encodeURIComponent(finalizedPayrollId)}`,
      ),
  });
}
