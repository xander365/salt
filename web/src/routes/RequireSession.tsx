// The route guard is convenience, never enforcement — the server refuses
// regardless of what this component does (docs/domain
// /operator-auth-http-web-grill.md, the browser route-guard clause under
// issue #59). It exists only so an Operator with no session lands on
// `/login` instead of a blank authenticated screen.

import type { ReactNode } from 'react';
import { Navigate } from 'react-router-dom';
import { useSession } from '../session/useSession';

export function RequireSession({ children }: { children: ReactNode }) {
  const session = useSession();

  if (session.isPending) {
    return <p>Loading…</p>;
  }

  if (session.isError) {
    return <Navigate to="/login" replace />;
  }

  return children;
}
