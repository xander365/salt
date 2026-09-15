// The four routes issue #63 wires up (parent #59 Spec 3 of 3, built server-
// side by #52): `POST .../compensation-terms`, `.../prior-employment`,
// `.../unsupported-deductions` and `.../opening-balance`. Everything an
// Employment needs to become payable.

import { useMutation, useQueryClient } from '@tanstack/react-query';
import { apiFetch } from '../api/client';
import { payrollRunsQueryKey } from '../payrollRuns/usePayrollRuns';
import type {
  DeclarePriorEmploymentRequest,
  DeclareUnsupportedDeductionStatusRequest,
  DivergingPeriodsResponse,
  EmploymentDetailResponse,
  RecordCompensationTermsRequest,
  RecordEmploymentEndDateRequest,
  RecordOpeningBalanceRequest,
  RecordedResponse,
} from '../api/types';
import { employmentQueryKey, useEmployerId } from './useEmployments';

/**
 * Every standing fact recorded here is one a payroll run's `blockers` are
 * derived from, on the server, on every read (§0.31). So the moment one is
 * recorded, any run detail this browser is still holding is known to be out
 * of date — not stale in the ordinary sense, but wrong in the one way this
 * screen must never be: telling an Operator to go and record a fact they
 * have just recorded (issue #64's own acceptance criteria). Nothing here
 * caches a blocker; this only throws away a whole run response that is no
 * longer true.
 */
function useFactRecorded() {
  const employerId = useEmployerId();
  const queryClient = useQueryClient();

  return (employmentId: string) => {
    void queryClient.invalidateQueries({
      queryKey: employmentQueryKey(employerId, employmentId),
    });
    void queryClient.invalidateQueries({ queryKey: payrollRunsQueryKey(employerId) });
  };
}

function employmentFactsUrl(employerId: string, employmentId: string, route: string): string {
  return `/api/employers/${encodeURIComponent(employerId)}/employments/${encodeURIComponent(employmentId)}/${route}`;
}

/** Recording pay never diverges from nothing here, so this screen sends
 * no `reason` and no `acknowledgedDivergingPeriods` — the server defaults
 * both to empty (issue #63's own scope: acknowledging a divergence from
 * already-run pay is a correction screen this ticket does not build). */
export function useRecordCompensationTerms(employmentId: string) {
  const employerId = useEmployerId();
  const factRecorded = useFactRecorded();

  return useMutation({
    mutationFn: (request: RecordCompensationTermsRequest) =>
      apiFetch<DivergingPeriodsResponse>(
        employmentFactsUrl(employerId, employmentId, 'compensation-terms'),
        { method: 'POST', body: JSON.stringify(request) },
      ),
    onSuccess: () => factRecorded(employmentId),
  });
}

export function useDeclarePriorEmployment(employmentId: string) {
  const employerId = useEmployerId();
  const factRecorded = useFactRecorded();

  return useMutation({
    mutationFn: (request: DeclarePriorEmploymentRequest) =>
      apiFetch<RecordedResponse>(employmentFactsUrl(employerId, employmentId, 'prior-employment'), {
        method: 'POST',
        body: JSON.stringify(request),
      }),
    onSuccess: () => factRecorded(employmentId),
  });
}

/** Same reason as {@link useRecordCompensationTerms} for omitting
 * `acknowledgedDivergingPeriods` — but `reason` is always sent here: the
 * server refuses a blank one unconditionally, divergence or not. */
export function useDeclareUnsupportedDeductionStatus(employmentId: string) {
  const employerId = useEmployerId();
  const factRecorded = useFactRecorded();

  return useMutation({
    mutationFn: (request: DeclareUnsupportedDeductionStatusRequest) =>
      apiFetch<DivergingPeriodsResponse>(
        employmentFactsUrl(employerId, employmentId, 'unsupported-deductions'),
        { method: 'POST', body: JSON.stringify(request) },
      ),
    onSuccess: () => factRecorded(employmentId),
  });
}

/** `PUT .../end-date` (issue #81): records that this Employment has a
 * Leaver's end date. This changes which future runs propose the Employment
 * at all, not merely a blocker on this one, so it invalidates the same two
 * queries every other stated fact here does. */
export function useRecordEmploymentEndDate(employmentId: string) {
  const employerId = useEmployerId();
  const factRecorded = useFactRecorded();

  return useMutation({
    mutationFn: (request: RecordEmploymentEndDateRequest) =>
      apiFetch<EmploymentDetailResponse>(employmentFactsUrl(employerId, employmentId, 'end-date'), {
        method: 'PUT',
        body: JSON.stringify(request),
      }),
    onSuccess: () => factRecorded(employmentId),
  });
}

export function useRecordOpeningBalance(employmentId: string) {
  const employerId = useEmployerId();
  const factRecorded = useFactRecorded();

  return useMutation({
    mutationFn: (request: RecordOpeningBalanceRequest) =>
      apiFetch<RecordedResponse>(employmentFactsUrl(employerId, employmentId, 'opening-balance'), {
        method: 'POST',
        body: JSON.stringify(request),
      }),
    onSuccess: () => factRecorded(employmentId),
  });
}
