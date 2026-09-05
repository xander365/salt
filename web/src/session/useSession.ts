// `GET /api/session` is the only source of truth about being signed in
// (§0.33). Nothing here caches an Operator or a membership list beyond
// TanStack Query's own cache of this one request — there is no second copy
// for a revoked session to go stale in.

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiFetch } from '../api/client';
import type { LoginRequest, SessionResponse } from '../api/types';

export const sessionQueryKey = ['session'] as const;

export function useSession() {
  return useQuery({
    queryKey: sessionQueryKey,
    queryFn: () => apiFetch<SessionResponse>('/api/session'),
    retry: false,
  });
}

export function useLogin() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: (credentials: LoginRequest) =>
      apiFetch<void>('/api/session', {
        method: 'POST',
        body: JSON.stringify(credentials),
      }),
    // The response carries no Operator state (§0.33) — invalidate so the
    // next read of `sessionQueryKey` goes back to the server for it.
    onSuccess: () => queryClient.invalidateQueries({ queryKey: sessionQueryKey }),
  });
}

export function useLogout() {
  const queryClient = useQueryClient();

  return useMutation({
    mutationFn: () => apiFetch<void>('/api/session', { method: 'DELETE' }),
    // Clears every cached query, not only the session one: the server row
    // is gone, so nothing this application holds about the prior Operator
    // is still true.
    onSuccess: () => queryClient.clear(),
  });
}
