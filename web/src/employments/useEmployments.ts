// `GET` and `POST /api/employers/{e}/employments` (issue #62, parent #59
// Spec 3 of 3, §0.38). The interface only ever sends `fullName` — the
// `personId` branch the API also accepts is never exercised from the
// browser, because there is no Person screen to have picked one from.

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useParams } from 'react-router-dom';
import { apiFetch } from '../api/client';
import type {
  CreateEmploymentRequest,
  CreateEmploymentResponse,
  EmploymentsResponse,
} from '../api/types';

function employmentsQueryKey(employerId: string) {
  return ['employers', employerId, 'employments'] as const;
}

// Every caller here lives beneath `/app/employers/:employerId`, so the param
// is always present — this just gives that guarantee a single, typed name
// instead of every call site re-reading `useParams()` and re-deciding what a
// missing value would mean.
function useEmployerId(): string {
  const { employerId } = useParams();
  if (employerId === undefined) {
    throw new Error('useEmployerId must be used inside an Employer-scoped route');
  }
  return employerId;
}

export function useEmployments() {
  const employerId = useEmployerId();

  return useQuery({
    queryKey: employmentsQueryKey(employerId),
    queryFn: () =>
      apiFetch<EmploymentsResponse>(
        `/api/employers/${encodeURIComponent(employerId)}/employments`,
      ),
  });
}

// Local calendar date, not `Date.toISOString()`'s UTC one — a start date
// picked near midnight must not land on the wrong day for the Operator's own
// timezone.
function todayAsIsoDate(): string {
  const now = new Date();
  const year = now.getFullYear();
  const month = String(now.getMonth() + 1).padStart(2, '0');
  const day = String(now.getDate()).padStart(2, '0');
  return `${year}-${month}-${day}`;
}

/**
 * Adds an Employment by full name only. `startDate` defaults to today and is
 * never asked of the Operator here — the API requires one, but this screen's
 * job is naming who the new employee is; everything else an Employment needs
 * is issue #63's own screen.
 *
 * Invalidates the list on success rather than pushing a local row: the
 * server's list is the truth, and a hand-maintained copy drifts (issue #62's
 * own Deep Instructions).
 */
export function useCreateEmployment() {
  const employerId = useEmployerId();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (fullName: string) => {
      const request: CreateEmploymentRequest = { fullName, startDate: todayAsIsoDate() };
      return apiFetch<CreateEmploymentResponse>(
        `/api/employers/${encodeURIComponent(employerId)}/employments`,
        { method: 'POST', body: JSON.stringify(request) },
      );
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: employmentsQueryKey(employerId) }),
  });
}
