// The route guard is convenience, never enforcement — the server refuses
// regardless of what this component does (docs/domain
// /operator-auth-http-web-grill.md, the browser route-guard clause under
// issue #59). It exists only so an Operator with no session lands on
// `/login` instead of a blank authenticated screen.

import type { ReactNode } from 'react';
import { Navigate } from 'react-router-dom';
import { ApiError } from '../api/client';
import { useSession } from '../session/useSession';

export function RequireSession({ children }: { children: ReactNode }) {
  const session = useSession();

  if (session.isPending) {
    return <p>Loading…</p>;
  }

  if (session.isError) {
    // Only the server's unauthenticated answer means the session is gone.
    // A 500 or transport failure tells us nothing about the Operator's
    // session and must remain retryable rather than posing as sign-out.
    if (session.error instanceof ApiError && session.error.status === 401) {
      return <Navigate to="/login" replace />;
    }

    return (
      <main>
        <p role="alert">We could not check your session. Please try again.</p>
        <button type="button" onClick={() => void session.refetch()}>
          Try again
        </button>
      </main>
    );
  }

  return children;
}
