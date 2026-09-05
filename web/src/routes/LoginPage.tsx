// `POST /api/session`. A wrong password and a locked account are
// indistinguishable from outside (§0.12a), so this page has exactly one
// failure message — it branches on `error.code`, never on `error.message`
// (§0.23), and every other failure gets a second, generic message rather
// than borrowing the credentials one.

import { type SubmitEvent, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { ApiError } from '../api/client';
import { useLogin } from '../session/useSession';

export function LoginPage() {
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const login = useLogin();
  const navigate = useNavigate();

  async function handleSubmit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);

    try {
      await login.mutateAsync({ email, password });
      navigate('/app', { replace: true });
    } catch (caught) {
      if (caught instanceof ApiError && caught.code === 'invalid_credentials') {
        setError('Incorrect email or password.');
      } else {
        setError('Something went wrong. Please try again.');
      }
    }
  }

  return (
    <main>
      <h1>Sign in</h1>
      <form onSubmit={handleSubmit}>
        <div>
          <label htmlFor="email">Email</label>
          <input
            id="email"
            name="email"
            type="email"
            autoComplete="username"
            required
            value={email}
            onChange={(event) => setEmail(event.target.value)}
          />
        </div>
        <div>
          <label htmlFor="password">Password</label>
          <input
            id="password"
            name="password"
            type="password"
            autoComplete="current-password"
            required
            value={password}
            onChange={(event) => setPassword(event.target.value)}
          />
        </div>
        {error !== null && <p role="alert">{error}</p>}
        <button type="submit" disabled={login.isPending}>
          {login.isPending ? 'Signing in…' : 'Sign in'}
        </button>
      </form>
    </main>
  );
}
