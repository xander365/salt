// The tracer bullet's whole authenticated surface (issue #60): proof that a
// browser can hold a real Salt session, sign out of it, and mean it. Every
// other screen is a later issue (#61 onward) — nothing here reads an
// Employer, because nothing here is authorized for one yet.

import { useNavigate } from 'react-router-dom';
import { useLogout, useSession } from '../session/useSession';

export function AppHome() {
  const session = useSession();
  const logout = useLogout();
  const navigate = useNavigate();

  async function handleSignOut() {
    await logout.mutateAsync();
    // Keep `/app` in history. Back must re-check the deleted server session
    // and return to `/login` on its resulting 401, never reveal cached UI.
    navigate('/login');
  }

  if (session.data === undefined) {
    return null;
  }

  return (
    <main>
      <p>Signed in as {session.data.operator.displayName}.</p>
      <button type="button" onClick={handleSignOut} disabled={logout.isPending}>
        Sign out
      </button>
    </main>
  );
}
