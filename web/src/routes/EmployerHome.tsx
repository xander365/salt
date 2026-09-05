// The Employer's own landing screen, inside the shell that names it. The
// screens this spec still owes it — `people`, `payroll`, `finalized` —
// arrive as further children of `EmployerShell` (issues #62 onward), each
// carrying `:employerId` in its own URL.

import { useAuthorizedSession } from '../session/AuthorizedSession';

export function EmployerHome() {
  const session = useAuthorizedSession();

  return (
    <main>
      <p>Signed in as {session.operator.displayName}.</p>
    </main>
  );
}
