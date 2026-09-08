// `GET`/`PUT /api/employers/{e}/people/{p}/particulars` and `PUT
// /api/employers/{e}/people/{p}/name` (issue #72, parent #70 D-7). Scoped by
// `personId`, not `employmentId`: `PersonParticulars` and a Person's
// `fullName` are facts about the Person, and the Employment screen reaches
// them through the `personId` its own `EmploymentDetailResponse` already
// carries.

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiFetch } from '../api/client';
import type {
  CorrectPersonFullNameRequest,
  DivergingPeriodsResponse,
  PersonParticularsResponse,
  SetPersonParticularsRequest,
} from '../api/types';
import { employmentsQueryKey, useEmployerId } from './useEmployments';

function personParticularsQueryKey(employerId: string, personId: string) {
  return ['employers', employerId, 'people', personId, 'particulars'] as const;
}

function personParticularsUrl(employerId: string, personId: string): string {
  return `/api/employers/${encodeURIComponent(employerId)}/people/${encodeURIComponent(personId)}/particulars`;
}

function personNameUrl(employerId: string, personId: string): string {
  return `/api/employers/${encodeURIComponent(employerId)}/people/${encodeURIComponent(personId)}/name`;
}

export function usePersonParticulars(personId: string) {
  const employerId = useEmployerId();

  return useQuery({
    queryKey: personParticularsQueryKey(employerId, personId),
    queryFn: () => apiFetch<PersonParticularsResponse>(personParticularsUrl(employerId, personId)),
  });
}

export function useSetPersonParticulars(personId: string) {
  const employerId = useEmployerId();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (request: SetPersonParticularsRequest) =>
      apiFetch<DivergingPeriodsResponse>(personParticularsUrl(employerId, personId), {
        method: 'PUT',
        body: JSON.stringify(request),
      }),
    onSuccess: () =>
      queryClient.invalidateQueries({
        queryKey: personParticularsQueryKey(employerId, personId),
      }),
  });
}

export function useCorrectPersonFullName(personId: string) {
  const employerId = useEmployerId();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (request: CorrectPersonFullNameRequest) =>
      apiFetch<DivergingPeriodsResponse>(personNameUrl(employerId, personId), {
        method: 'PUT',
        body: JSON.stringify(request),
      }),
    onSuccess: () =>
      Promise.all([
        queryClient.invalidateQueries({
          queryKey: personParticularsQueryKey(employerId, personId),
        }),
        // A Person's name is repeated in the People list and every Employment
        // detail for that Person. Invalidate the whole Employment prefix so
        // no screen keeps presenting the misspelling after it was corrected.
        queryClient.invalidateQueries({ queryKey: employmentsQueryKey(employerId) }),
      ]),
  });
}
