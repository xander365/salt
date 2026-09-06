// The four routes issue #63 wires up (parent #59 Spec 3 of 3, built server-
// side by #52): `POST .../compensation-terms`, `.../prior-employment`,
// `.../unsupported-deductions` and `.../opening-balance`. Everything an
// Employment needs to become payable.

import { useMutation, useQueryClient } from '@tanstack/react-query';
import { apiFetch } from '../api/client';
import type {
  DeclarePriorEmploymentRequest,
  DeclareUnsupportedDeductionStatusRequest,
  DivergingPeriodsResponse,
  RecordCompensationTermsRequest,
  RecordOpeningBalanceRequest,
  RecordedResponse,
} from '../api/types';
import { employmentQueryKey, useEmployerId } from './useEmployments';

function employmentFactsUrl(employerId: string, employmentId: string, route: string): string {
  return `/api/employers/${encodeURIComponent(employerId)}/employments/${encodeURIComponent(employmentId)}/${route}`;
}

/** Recording pay never diverges from nothing here, so this screen sends
 * no `reason` and no `acknowledgedDivergingPeriods` — the server defaults
 * both to empty (issue #63's own scope: acknowledging a divergence from
 * already-run pay is a correction screen this ticket does not build). */
export function useRecordCompensationTerms(employmentId: string) {
  const employerId = useEmployerId();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (request: RecordCompensationTermsRequest) =>
      apiFetch<DivergingPeriodsResponse>(
        employmentFactsUrl(employerId, employmentId, 'compensation-terms'),
        { method: 'POST', body: JSON.stringify(request) },
      ),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: employmentQueryKey(employerId, employmentId) }),
  });
}

export function useDeclarePriorEmployment(employmentId: string) {
  const employerId = useEmployerId();

  return useMutation({
    mutationFn: (request: DeclarePriorEmploymentRequest) =>
      apiFetch<RecordedResponse>(employmentFactsUrl(employerId, employmentId, 'prior-employment'), {
        method: 'POST',
        body: JSON.stringify(request),
      }),
  });
}

/** Same reason as {@link useRecordCompensationTerms} for omitting
 * `acknowledgedDivergingPeriods` — but `reason` is always sent here: the
 * server refuses a blank one unconditionally, divergence or not. */
export function useDeclareUnsupportedDeductionStatus(employmentId: string) {
  const employerId = useEmployerId();

  return useMutation({
    mutationFn: (request: DeclareUnsupportedDeductionStatusRequest) =>
      apiFetch<DivergingPeriodsResponse>(
        employmentFactsUrl(employerId, employmentId, 'unsupported-deductions'),
        { method: 'POST', body: JSON.stringify(request) },
      ),
  });
}

export function useRecordOpeningBalance(employmentId: string) {
  const employerId = useEmployerId();

  return useMutation({
    mutationFn: (request: RecordOpeningBalanceRequest) =>
      apiFetch<RecordedResponse>(employmentFactsUrl(employerId, employmentId, 'opening-balance'), {
        method: 'POST',
        body: JSON.stringify(request),
      }),
  });
}
