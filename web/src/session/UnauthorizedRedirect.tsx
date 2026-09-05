// Mounted once at the root of the router. A 401 from any request sends the
// application to the sign-in page (§0.33) — this is what turns the fetch
// wrapper's `onUnauthorized` event into an actual navigation, since
// `src/api/client.ts` has no router of its own to call.

import { useEffect } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { useQueryClient } from '@tanstack/react-query';
import { onUnauthorized } from '../api/client';
import { sessionQueryKey } from './useSession';

export const LOGIN_PATH = '/login';

export function UnauthorizedRedirect() {
  const navigate = useNavigate();
  const location = useLocation();
  const queryClient = useQueryClient();
  const pathname = location.pathname;

  useEffect(
    () =>
      onUnauthorized(() => {
        // Mark the cached session stale without asking for it again.
        // A 401 has already told us the answer, so the default `active`
        // refetch would only send a second request certain to be refused,
        // and each refusal dispatches this event again.
        queryClient.invalidateQueries({ queryKey: sessionQueryKey, refetchType: 'none' });

        // A 401 on the sign-in page is not a navigation. `POST /api/session`
        // answers a wrong password with 401 `invalid_credentials`, and that
        // is the page doing its job, not a session ending — redirecting on it
        // would replace `/login` with itself and put a spurious entry in the
        // history the Back-button behaviour is measured against.
        if (pathname === LOGIN_PATH) {
          return;
        }

        navigate(LOGIN_PATH, { replace: true });
      }),
    [navigate, pathname, queryClient],
  );

  return null;
}
