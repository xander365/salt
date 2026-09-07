// The one "not found" screen (issue #61, ADR-0017/§0.10): an Employer id
// outside the caller's own memberships and an Employer id that names no row
// at all render identically. The server already collapses both to a 404
// rather than a 403 so the URL cannot be used to probe which ids exist; this
// screen is that same refusal to distinguish, carried into the UI. It says
// nothing about permission, and offers no way back that would only be
// offered to an Operator who guessed a real id.

import { SearchX } from 'lucide-react';

export function NotFound() {
  return (
    <main className="mx-auto flex min-h-dvh max-w-md flex-col items-center justify-center gap-3 px-6 text-center">
      <SearchX className="size-10 text-muted-foreground" aria-hidden="true" />
      <h1 className="text-xl font-semibold tracking-tight">Not found</h1>
      <p className="text-muted-foreground">We could not find that.</p>
    </main>
  );
}
