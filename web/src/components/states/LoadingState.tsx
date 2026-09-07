// The shared loading state (issue #88): a screen or a section that is
// waiting on its first `GET` shows this rather than improvising its own
// "Loading…" text. `role="status"` — not `alert` — because arriving here is
// expected, not a change worth interrupting a screen reader for.

import { Loader2 } from 'lucide-react';

export function LoadingState({ label = 'Loading…' }: { label?: string }) {
  return (
    <p role="status" className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
      <Loader2 className="size-4 animate-spin" aria-hidden="true" />
      {label}
    </p>
  );
}
