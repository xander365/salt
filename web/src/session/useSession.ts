// `GET /api/session` is the only source of truth about being signed in
// (§0.33). Nothing here caches an Operator or a membership list beyond
// TanStack Query's own cache of this one request — there is no second copy
// for a revoked session to go stale in.

import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from 'react-router-dom';
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

/**
 * Signing out is three steps in one order that matters, so it is one thing a
 * screen calls rather than three a screen could get wrong:
 *
 * 1. `DELETE /api/session`, so the server row is gone and revocation bites on
 *    the next request (§0.8) rather than only in this tab.
 * 2. leave the guarded screen,
 * 3. empty the query cache — nothing it holds about the prior Operator is
 *    still true.
 *
 * Steps 2 and 3 are adjacent and synchronous on purpose. Clearing first lets
 * React re-render the still-mounted `RequireSession` with no session data,
 * which refetches `GET /api/session`, earns a `401`, and fires
 * `UnauthorizedRedirect`'s `replace: true` navigation. That would drop `/app`
 * out of history and so hide the Back-button behaviour this flow exists to
 * make provable: Back must reach `/app`, make a real request, and be refused.
 */
export function useSignOut() {
  const queryClient = useQueryClient();
  const navigate = useNavigate();

  const mutation = useMutation({
    mutationFn: () => apiFetch<void>('/api/session', { method: 'DELETE' }),
  });

  return {
    isPending: mutation.isPending,
    signOut: async () => {
      await mutation.mutateAsync();
      navigate('/login');
      queryClient.clear();
    },
  };
}
