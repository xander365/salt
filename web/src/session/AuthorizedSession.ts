// The loaded `GET /api/session` body, handed down from `RequireSession` to
// the screens it guards.
//
// This is **not** a second copy of "who is signed in" (§0.33, and the spec's
// own second-most-likely failure). The value published here is the very
// object TanStack Query holds for `sessionQueryKey`, re-read from
// `useSession()` on every render of the guard and never written to, so a
// revoked session still bites on the next request. It exists for one reason:
// a screen behind the guard cannot be reached before the query resolved, so
// it must not carry a `data === undefined` branch — and a branch like that
// renders blank, which is exactly what §49 forbids.

import { createContext, useContext } from 'react';
import type { SessionResponse } from '../api/types';

const AuthorizedSessionContext = createContext<SessionResponse | null>(null);

export const AuthorizedSessionProvider = AuthorizedSessionContext.Provider;

export function useAuthorizedSession(): SessionResponse {
  const session = useContext(AuthorizedSessionContext);

  if (session === null) {
    throw new Error('useAuthorizedSession must be used inside RequireSession');
  }

  return session;
}
