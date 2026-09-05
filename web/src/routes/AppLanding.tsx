// `/app` (issue #61, §0.34): one membership means redirect straight into it,
// several means a plain list — never an Employer switcher widget, since the
// URL already carries the EmployerId and a switcher would add nothing behind
// it. Reads the memberships `GET /api/session` already returned; there is no
// second call to make here.

import { Link, Navigate } from 'react-router-dom';
import { useSession } from '../session/useSession';

export function AppLanding() {
  const session = useSession();

  if (session.data === undefined) {
    return null;
  }

  const { memberships } = session.data;

  if (memberships.length === 1) {
    return <Navigate to={`/app/employers/${memberships[0].employerId}`} replace />;
  }

  return (
    <main>
      <h1>Choose an Employer</h1>
      {memberships.length === 0 ? (
        <p>You have no Employers yet.</p>
      ) : (
        <ul>
          {memberships.map((membership) => (
            <li key={membership.employerId}>
              <Link to={`/app/employers/${membership.employerId}`}>{membership.name}</Link>
            </li>
          ))}
        </ul>
      )}
    </main>
  );
}
