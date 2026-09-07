// `POST /api/session`. A wrong password and a locked account are
// indistinguishable from outside (§0.12a), so this page has exactly one
// failure message — it branches on `error.code`, never on `error.message`
// (§0.23), and every other failure gets a second, generic message rather
// than borrowing the credentials one.

import { type SubmitEvent, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { ApiError } from '../api/client';
import { useLogin } from '../session/useSession';
import { Button } from '../components/ui/button';
import { Input } from '../components/ui/input';
import { Label } from '../components/ui/label';
import { ValidationError } from '../components/states/ValidationError';

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
    <main className="mx-auto flex min-h-dvh max-w-sm flex-col justify-center px-6 py-16">
      <h1 className="mb-6 text-xl font-semibold tracking-tight">Sign in</h1>
      <form onSubmit={handleSubmit} className="flex flex-col gap-4">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="email">Email</Label>
          <Input
            id="email"
            name="email"
            type="email"
            autoComplete="username"
            required
            value={email}
            onChange={(event) => setEmail(event.target.value)}
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="password">Password</Label>
          <Input
            id="password"
            name="password"
            type="password"
            autoComplete="current-password"
            required
            value={password}
            onChange={(event) => setPassword(event.target.value)}
          />
        </div>
        {error !== null && <ValidationError>{error}</ValidationError>}
        <Button type="submit" disabled={login.isPending} className="mt-2">
          {login.isPending ? 'Signing in…' : 'Sign in'}
        </Button>
      </form>
    </main>
  );
}
