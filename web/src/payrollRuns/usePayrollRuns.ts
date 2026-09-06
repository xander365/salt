// `POST`, `GET /api/employers/{e}/payroll-runs` and `GET
// .../payroll-runs/{r}` (issue #64, parent #59 Spec 3 of 3, §0.22/§0.31). An
// Employer's payroll runs, and every member one run proposes to pay.

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiFetch } from '../api/client';
import type {
  CreatePayrollRunRequest,
  CreatePayrollRunResponse,
  PayrollRunDetailResponse,
  PayrollRunsResponse,
} from '../api/types';
import { useEmployerId } from '../employments/useEmployments';

function payrollRunsQueryKey(employerId: string) {
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
