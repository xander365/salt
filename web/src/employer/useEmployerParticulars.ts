// `GET`/`PUT /api/employers/{e}/particulars` (issue #71, parent #70 D-7).

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiFetch } from '../api/client';
import { useEmployerId } from '../employments/useEmployments';
import type {
  DivergingPeriodsResponse,
  EmployerParticularsResponse,
  SetEmployerParticularsRequest,
} from '../api/types';

function employerParticularsQueryKey(employerId: string) {
  return ['employers', employerId, 'particulars'] as const;
}

function employerParticularsUrl(employerId: string): string {
  return `/api/employers/${encodeURIComponent(employerId)}/particulars`;
}

/** `null` when this Employer has never recorded particulars yet — not an
 * error, and never a 404 (the Employer itself is what the URL names). */
export function useEmployerParticulars() {
  const employerId = useEmployerId();

  return useQuery({
    queryKey: employerParticularsQueryKey(employerId),
    queryFn: () => apiFetch<EmployerParticularsResponse | null>(employerParticularsUrl(employerId)),
  });
}

export function useSetEmployerParticulars() {
  const employerId = useEmployerId();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (request: SetEmployerParticularsRequest) =>
      apiFetch<DivergingPeriodsResponse>(employerParticularsUrl(employerId), {
        method: 'PUT',
        body: JSON.stringify(request),
      }),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: employerParticularsQueryKey(employerId) }),
  });
}
