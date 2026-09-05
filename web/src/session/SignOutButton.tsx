// Sign out, wherever an Operator is standing. It lives beside the Employer
// name in the application shell and on the Employer list, so no authorized
// screen is one an Operator cannot leave from.

import { useState } from 'react';
import { useSignOut } from './useSession';

export function SignOutButton() {
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

  return (
    <>
      {failed && <p role="alert">We could not sign you out. Please try again.</p>}
      <button type="button" onClick={() => void handleSignOut()} disabled={isPending}>
        {isPending ? 'Signing out…' : 'Sign out'}
      </button>
    </>
  );
}
