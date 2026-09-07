// The route guard is convenience, never enforcement — the server refuses
// regardless of what this component does (docs/domain
// /operator-auth-http-web-grill.md, the browser route-guard clause under
// issue #59). It exists only so an Operator with no session lands on
// `/login` instead of a blank authenticated screen.
//
// It is a layout route, so every authorized screen is a child of it by
// construction rather than by each route element remembering to wrap
// itself. Its children render only once the session query has resolved, and
// they read it from `useAuthorizedSession()` — so no screen behind this
// guard owns a "session not loaded yet" branch, and none of them can render
// blank.

import { Navigate, Outlet } from 'react-router-dom';
import { ApiError } from '../api/client';
import { AuthorizedSessionProvider } from '../session/AuthorizedSession';
import { LOGIN_PATH } from '../session/UnauthorizedRedirect';
import { useSession } from '../session/useSession';
import { LoadingState } from '../components/states/LoadingState';
import { FailedRequestState } from '../components/states/FailedRequestState';

export function RequireSession() {
  const session = useSession();

  if (session.isPending) {
    return (
      <main className="flex min-h-dvh items-center justify-center">
        <LoadingState />
      </main>
    );
  }

  if (session.isError) {
    // Only the server's unauthenticated answer means the session is gone.
    // A 500 or transport failure tells us nothing about the Operator's
    // session and must remain retryable rather than posing as sign-out
    // (§49: a screen that failed to load says so and offers a retry).
    if (session.error instanceof ApiError && session.error.status === 401) {
      return <Navigate to={LOGIN_PATH} replace />;
    }

    return (
      <main className="flex min-h-dvh items-center justify-center px-6">
        <FailedRequestState
          message="We could not check your session. Please try again."
          onRetry={() => void session.refetch()}
          retrying={session.isFetching}
        />
      </main>
    );
  }

  return (
    <AuthorizedSessionProvider value={session.data}>
      <Outlet />
    </AuthorizedSessionProvider>
  );
}
