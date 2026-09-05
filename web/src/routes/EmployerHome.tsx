// `/app/:employerId` (issue #61, parent #59 Spec 3). The application shell
// for every authorized screen: it names the Employer the Operator is about
// to touch, from the membership `GET /api/session` already returned for it —
// no separate call to look the Employer up.
//
// The URL is navigation, never permission (§0.8): nothing here grants
// access to `:employerId`, so an id outside the Operator's own memberships
// renders `NotFound`, identically to one that names no Employer at all. The
// server re-checks membership on every request regardless of what this
// component decides.

import { useState } from 'react';
import { useParams } from 'react-router-dom';
import { useSession, useSignOut } from '../session/useSession';
import { NotFound } from './NotFound';

export function EmployerHome() {
  const { employerId } = useParams();
  const session = useSession();
  const { isPending, signOut } = useSignOut();
  const [failed, setFailed] = useState(false);

  async function handleSignOut() {
    setFailed(false);

    try {
      await signOut();
    } catch {
      // Signing out has to take effect on the server, not only in this tab.
      // If the server never confirmed it, saying nothing would leave an
      // Operator walking away from a shared machine still signed in.
      setFailed(true);
    }
  }

  if (session.data === undefined) {
    return null;
  }

  const membership = session.data.memberships.find(
    (candidate) => candidate.employerId === employerId,
  );

  if (membership === undefined) {
    return <NotFound />;
  }

  return (
    <>
      <header>
        <p>{membership.name}</p>
      </header>
      <main>
        <p>Signed in as {session.data.operator.displayName}.</p>
        {failed && <p role="alert">We could not sign you out. Please try again.</p>}
        <button type="button" onClick={() => void handleSignOut()} disabled={isPending}>
          {isPending ? 'Signing out…' : 'Sign out'}
        </button>
      </main>
    </>
  );
}
