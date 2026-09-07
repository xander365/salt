// The shared empty state (issue #88): a list with nothing in it yet says so
// plainly, in the same shape everywhere — never a screen-specific one-off
// sentence styled differently from its neighbours.

import type { ReactNode } from 'react';
import { Inbox } from 'lucide-react';

export function EmptyState({ children }: { children: ReactNode }) {
  return (
    <div className="flex flex-col items-center gap-2 rounded-lg border border-dashed py-10 text-center text-sm text-muted-foreground">
      <Inbox className="size-8" aria-hidden="true" />
      <p>{children}</p>
    </div>
  );
}
