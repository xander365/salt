// `GET`/`POST .../employments/{em}/standing-pay-items` and `POST
// .../standing-pay-items/{s}/end` (issue #79). An Employment's standing
// allowances and medical aid premiums, recorded once and proposed by every
// new Ordinary run.

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiFetch } from '../api/client';
import type {
  CreateStandingPayItemRequest,
  CreateStandingPayItemResponse,
  EndStandingPayItemRequest,
  RecordedResponse,
  StandingPayItemsResponse,
} from '../api/types';
import { employmentQueryKey, useEmployerId } from './useEmployments';

export function standingPayItemsQueryKey(employerId: string, employmentId: string) {
  return [...employmentQueryKey(employerId, employmentId), 'standing-pay-items'] as const;
}

function standingPayItemsUrl(employerId: string, employmentId: string): string {
  return `/api/employers/${encodeURIComponent(employerId)}/employments/${encodeURIComponent(employmentId)}/standing-pay-items`;
}

export function useStandingPayItems(employmentId: string) {
  const employerId = useEmployerId();

  return useQuery({
    queryKey: standingPayItemsQueryKey(employerId, employmentId),
    queryFn: () =>
      apiFetch<StandingPayItemsResponse>(standingPayItemsUrl(employerId, employmentId)),
  });
}

/** Recording or ending an item changes no draft run: a draft holds the
 * proposal it was created with, never a live view (§D-6). Only this
 * Employment's own list is re-read. */
function useItemsChanged(employmentId: string) {
  const employerId = useEmployerId();
  const queryClient = useQueryClient();

  return () =>
    void queryClient.invalidateQueries({
      queryKey: standingPayItemsQueryKey(employerId, employmentId),
    });
}

export function useCreateStandingPayItem(employmentId: string) {
  const employerId = useEmployerId();
  const itemsChanged = useItemsChanged(employmentId);

  return useMutation({
    mutationFn: (request: CreateStandingPayItemRequest) =>
      apiFetch<CreateStandingPayItemResponse>(standingPayItemsUrl(employerId, employmentId), {
        method: 'POST',
        body: JSON.stringify(request),
      }),
    onSuccess: itemsChanged,
  });
}

export function useEndStandingPayItem(employmentId: string) {
  const employerId = useEmployerId();
  const itemsChanged = useItemsChanged(employmentId);

  return useMutation({
    mutationFn: ({
      standingPayItemId,
      ...request
    }: EndStandingPayItemRequest & { standingPayItemId: string }) =>
      apiFetch<RecordedResponse>(
        `${standingPayItemsUrl(employerId, employmentId)}/${encodeURIComponent(standingPayItemId)}/end`,
        { method: 'POST', body: JSON.stringify(request) },
      ),
    onSuccess: itemsChanged,
  });
}
