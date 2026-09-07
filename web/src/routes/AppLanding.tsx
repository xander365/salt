// `/app` (issue #61, §0.34): one membership means redirect straight into it,
// several means a plain list — never an Employer switcher widget, since the
// URL already carries the EmployerId and a switcher would add nothing behind
// it. Reads the memberships `GET /api/session` already returned; there is no
// second call to make here.

import { Building2 } from 'lucide-react';
import { useQueryClient } from '@tanstack/react-query';
import { Link, Navigate } from 'react-router-dom';
import { useAuthorizedSession } from '../session/AuthorizedSession';
import { SignOutButton } from '../session/SignOutButton';
import { Card, CardHeader, CardTitle } from '../components/ui/card';
import { EmptyState } from '../components/states/EmptyState';
import { employerPath } from './paths';

export function AppLanding() {
  const { memberships } = useAuthorizedSession();
  const queryClient = useQueryClient();

  function clearEmployerCache() {
    queryClient.removeQueries({ queryKey: ['employers'] });
  }

  if (memberships.length === 1) {
    return <Navigate to={employerPath(memberships[0].employerId)} replace />;
  }

  return (
    <main className="mx-auto flex min-h-dvh max-w-md flex-col gap-6 px-6 py-16">
      <div className="flex items-center justify-between">
        <h1 className="text-xl font-semibold tracking-tight">Choose an Employer</h1>
        <SignOutButton />
      </div>

      {memberships.length === 0 ? (
        <EmptyState>You have no Employers yet.</EmptyState>
      ) : (
        <ul className="flex flex-col gap-3">
          {memberships.map((membership) => (
            <li key={membership.employerId}>
              <Link to={employerPath(membership.employerId)} onClick={clearEmployerCache}>
                <Card className="transition-colors hover:border-primary">
                  <CardHeader className="flex-row items-center gap-3 space-y-0">
                    <Building2 className="size-5 text-primary" aria-hidden="true" />
                    <CardTitle>{membership.name}</CardTitle>
                  </CardHeader>
                </Card>
              </Link>
            </li>
          ))}
        </ul>
      )}
    </main>
  );
}
