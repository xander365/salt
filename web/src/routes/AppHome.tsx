// The tracer bullet's whole authenticated surface (issue #60): proof that a
// browser can hold a real Salt session, sign out of it, and mean it. Every
// other screen is a later issue (#61 onward) — nothing here reads an
// Employer, because nothing here is authorized for one yet.

import { useState } from 'react';
import { useSession, useSignOut } from '../session/useSession';

export function AppHome() {
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

  return (
    <main>
      <p>Signed in as {session.data.operator.displayName}.</p>
      {failed && <p role="alert">We could not sign you out. Please try again.</p>}
      <button type="button" onClick={() => void handleSignOut()} disabled={isPending}>
        {isPending ? 'Signing out…' : 'Sign out'}
      </button>
    </main>
  );
}
