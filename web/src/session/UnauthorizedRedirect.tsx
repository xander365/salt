// Mounted once at the root of the router. A 401 from any request sends the
// application to the sign-in page (§0.33) — this is what turns the fetch
// wrapper's `onUnauthorized` event into an actual navigation, since
// `src/api/client.ts` has no router of its own to call.

import { useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { useQueryClient } from '@tanstack/react-query';
import { onUnauthorized } from '../api/client';
import { sessionQueryKey } from './useSession';

export function UnauthorizedRedirect() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();

  useEffect(
    () =>
      onUnauthorized(() => {
        queryClient.invalidateQueries({ queryKey: sessionQueryKey });
        navigate('/login', { replace: true });
      }),
    [navigate, queryClient],
  );

  return null;
}
