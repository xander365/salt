// The Employer's own landing screen, inside the shell that names it. `people`
// is now a sibling route (issue #62); `payroll` and `finalized` still arrive
// as further children of `EmployerShell` (issue #63 onward), each carrying
// `:employerId` in its own URL.

import { Link } from 'react-router-dom';
import { useAuthorizedSession } from '../session/AuthorizedSession';

export function EmployerHome() {
  const session = useAuthorizedSession();

  return (
    <main>
      <p>Signed in as {session.operator.displayName}.</p>
      {/* Relative, not an absolute path rebuilt from `:employerId`: this
          screen already renders beneath `/app/employers/:employerId`, so
          resolving against the current route needs no second read of the
          same param — and cannot disagree with it. */}
      <p>
        <Link to="people">People</Link>
      </p>
    </main>
  );
}
