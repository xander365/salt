// `POST`, `GET /api/employers/{e}/payroll-runs` and `GET
// .../payroll-runs/{r}` (issue #64, parent #59 Spec 3 of 3, §0.22/§0.31). An
// Employer's payroll runs, and every member one run proposes to pay.

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiFetch } from '../api/client';
import type {
  CreatePayrollRunRequest,
  CreatePayrollRunResponse,
  EarningLineDto,
  PayrollRunDetailResponse,
  PayrollRunsResponse,
  RecordedResponse,
} from '../api/types';
import { useEmployerId } from '../employments/useEmployments';

/** Every payroll-run query for one Employer — the list and each run's own
 * detail alike, since a detail key extends this one. Invalidating here is
 * how a change made elsewhere says "what a run screen is holding is now
 * out of date". */
export function payrollRunsQueryKey(employerId: string) {
  return ['employers', employerId, 'payrollRuns'] as const;
}

export function payrollRunQueryKey(employerId: string, payrollRunId: string) {
  return ['employers', employerId, 'payrollRuns', payrollRunId] as const;
}

export function usePayrollRuns() {
  const employerId = useEmployerId();

  return useQuery({
    queryKey: payrollRunsQueryKey(employerId),
    queryFn: () =>
      apiFetch<PayrollRunsResponse>(
        `/api/employers/${encodeURIComponent(employerId)}/payroll-runs`,
      ),
  });
}

// `GET /api/employers/{e}/payroll-runs/{r}`: every member this run proposes
// to pay, with their `blockers` derived fresh on this same read (§0.31) —
// never a copy cached beside it, so a fact recorded on the Employment screen
// and this screen reloaded always agree.
export function usePayrollRun(payrollRunId: string) {
  const employerId = useEmployerId();

  return useQuery({
    queryKey: payrollRunQueryKey(employerId, payrollRunId),
    queryFn: () =>
      apiFetch<PayrollRunDetailResponse>(
        `/api/employers/${encodeURIComponent(employerId)}/payroll-runs/${encodeURIComponent(payrollRunId)}`,
      ),
  });
}

/**
 * Creates an Ordinary run only (issue #64's own scope: no Correction-run
 * creation form here, the same boundary `payroll_runs.rs`'s own create route
 * draws). Invalidates the list on success rather than growing it locally —
 * the same reason `useCreateEmployment` does (issue #62).
 */
export function useCreatePayrollRun() {
  const employerId = useEmployerId();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (request: CreatePayrollRunRequest) =>
      apiFetch<CreatePayrollRunResponse>(
        `/api/employers/${encodeURIComponent(employerId)}/payroll-runs`,
        { method: 'POST', body: JSON.stringify(request) },
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: payrollRunsQueryKey(employerId) }),
  });
}

/**
 * `PUT .../payroll-runs/{r}/members/{em}/earnings` (issue #65): replaces one
 * member's whole Earnings list, matching `set_run_earnings`'s own contract —
 * the caller sends every line every time, never a delta.
 *
 * Invalidates the run detail on success rather than patching it locally.
 * That refetch is a plain `GET`, which always answers `refusal: null` for
 * every member (§0.31) — so editing one member's earnings honestly retires
 * whatever the last Calculate said about every member, not just this one.
 * Nothing here remembers a `refusal` past the response that carried it.
 */
export function useSetRunEarnings(payrollRunId: string) {
  const employerId = useEmployerId();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: ({
      employmentId,
      earnings,
    }: {
      employmentId: string;
      earnings: EarningLineDto[];
    }) =>
      apiFetch<RecordedResponse>(
        `/api/employers/${encodeURIComponent(employerId)}/payroll-runs/${encodeURIComponent(payrollRunId)}/members/${encodeURIComponent(employmentId)}/earnings`,
        { method: 'PUT', body: JSON.stringify({ earnings }) },
      ),
    onSuccess: () =>
      queryClient.invalidateQueries({ queryKey: payrollRunQueryKey(employerId, payrollRunId) }),
  });
}

/**
 * `POST .../payroll-runs/{r}/calculate` (issue #55/#65). Calculate is not an
 * error (§0.25): a `200` here may still carry a refused member, and that is
 * this hook's ordinary success path, not a thrown `ApiError`.
 *
 * Writes the response straight into the run detail's own cache entry rather
 * than invalidating it — the response already *is* that same detail, with
 * one thing a later plain `GET` could never carry: each refused member's
 * fresh `refusal` (§0.31). Refetching instead of writing it directly would
 * throw that away the instant it arrived.
 */
export function useCalculatePayrollRun(payrollRunId: string) {
  const employerId = useEmployerId();
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: () =>
      apiFetch<PayrollRunDetailResponse>(
        `/api/employers/${encodeURIComponent(employerId)}/payroll-runs/${encodeURIComponent(payrollRunId)}/calculate`,
        { method: 'POST' },
      ),
    onSuccess: (data) =>
      queryClient.setQueryData(payrollRunQueryKey(employerId, payrollRunId), data),
  });
}
