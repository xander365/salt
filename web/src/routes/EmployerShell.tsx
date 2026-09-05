// The application shell for every authorized Employer screen
// (`/app/employers/:employerId/*`, issue #61, parent #59 Spec 3).
//
// It is a layout route on purpose. The Employer's name has to be visible on
// *every* authorized screen, and a layout route makes that structural: a
// screen added under this path inherits the name and the sign-out control
// without knowing they exist, and cannot forget them. The name comes from
// the membership `GET /api/session` already returned, so there is no second
// call to look the Employer up.
//
// The URL is navigation, never permission (§0.8): nothing here grants
// access to `:employerId`. An id outside the Operator's own memberships
// renders `NotFound`, identically to one that names no Employer at all
// (§0.10) — and the server re-checks membership on every request regardless
// of what this component decides.

import { Outlet, useParams } from 'react-router-dom';
import { useAuthorizedSession } from '../session/AuthorizedSession';
import { SignOutButton } from '../session/SignOutButton';
import { NotFound } from './NotFound';

export function EmployerShell() {
  const { employerId } = useParams();
  const session = useAuthorizedSession();

  const membership = session.memberships.find(
    (candidate) => candidate.employerId === employerId,
  );

  if (membership === undefined) {
    return <NotFound />;
  }

  return (
    <>
      <header>
        <h1>{membership.name}</h1>
        <SignOutButton />
      </header>
      <Outlet />
    </>
  );
}
