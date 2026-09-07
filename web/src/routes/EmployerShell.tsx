// The application shell for every authorized Employer screen
// (`/app/employers/:employerId/*`, issue #61, parent #59 Spec 3; rebuilt for
// issue #88's persistent navigation).
//
// It is a layout route on purpose. The Employer's name, the persistent
// People / Payroll navigation, and the way out have to be on *every*
// authorized screen, and a layout route makes that structural: a screen
// added under this path inherits all three without knowing they exist, and
// cannot forget them. The name comes from the membership `GET /api/session`
// already returned, so there is no second call to look the Employer up.
//
// The URL is navigation, never permission (§0.8): nothing here grants
// access to `:employerId`. An id outside the Operator's own memberships
// renders `NotFound`, identically to one that names no Employer at all
// (§0.10) — and the server re-checks membership on every request regardless
// of what this component decides.

import { NavLink, Outlet, useParams } from 'react-router-dom';
import { cn } from '../lib/utils';
import { useAuthorizedSession } from '../session/AuthorizedSession';
import { SignOutButton } from '../session/SignOutButton';
import { Badge } from '../components/ui/badge';
import { NotFound } from './NotFound';

const ROLE_LABEL: Record<string, string> = {
  owner: 'Owner',
  payrollOperator: 'Payroll operator',
};

function navLinkClassName({ isActive }: { isActive: boolean }): string {
  return cn(
    'rounded-md px-3 py-1.5 text-sm font-medium transition-colors',
    isActive
      ? 'bg-primary text-primary-foreground'
      : 'text-muted-foreground hover:bg-accent hover:text-accent-foreground',
  );
}

export function EmployerShell() {
  const { employerId } = useParams();
  const session = useAuthorizedSession();

  const membership = session.memberships.find((candidate) => candidate.employerId === employerId);

  if (membership === undefined) {
    return <NotFound />;
  }

  return (
    <div className="min-h-dvh bg-background">
      <header className="border-b bg-card">
        <div className="mx-auto flex max-w-6xl flex-wrap items-center justify-between gap-3 px-6 py-4">
          <div className="flex items-center gap-3">
            {/* The active Employer, always named (issue #88's own acceptance
                criterion) — level 1, because it is the one heading that
                names what every screen beneath it is scoped to. Linked to
                this Employer's own Overview: the "Employer" third of D13's
                People / Payroll / Employer navigation. */}
            <h1 className="text-lg font-semibold tracking-tight">
              <NavLink to="" end className="hover:underline">
                {membership.name}
              </NavLink>
            </h1>
            <Badge variant="outline">{ROLE_LABEL[membership.role] ?? membership.role}</Badge>
          </div>
          <div className="flex items-center gap-2">
            {session.memberships.length > 1 && (
              // Not a switcher widget (issue #61's own decision, preserved):
              // a plain link back to the Employer list, the same navigation
              // an Operator with one Employer never needs to see.
              <NavLink
                to="/app"
                className="text-sm font-medium text-muted-foreground hover:text-foreground hover:underline"
              >
                Switch employer
              </NavLink>
            )}
            <SignOutButton />
          </div>
        </div>
        <nav aria-label="Primary" className="mx-auto flex max-w-6xl gap-1 px-6 pb-3">
          <NavLink to="people" className={navLinkClassName}>
            People
          </NavLink>
          <NavLink to="payroll" className={navLinkClassName}>
            Payroll
          </NavLink>
        </nav>
      </header>
      <div className="mx-auto max-w-6xl px-6 py-8">
        <Outlet />
      </div>
    </div>
  );
}
