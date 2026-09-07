// The shared stale state (issue #88): the figures on screen were true of an
// earlier Calculate, and something has moved since — either a background
// refetch is in flight, or this member's earnings were saved after the
// figures shown were last calculated. Never colour alone: an icon and a
// sentence say what changed and what to do.

import { Clock } from 'lucide-react';

export function StaleBanner({ children }: { children: string }) {
  return (
    <p
      role="status"
      className="flex items-center gap-2 rounded-md border border-muted-foreground/30 bg-muted px-3 py-2 text-sm text-muted-foreground"
    >
      <Clock className="size-4 shrink-0" aria-hidden="true" />
      {children}
    </p>
  );
}
