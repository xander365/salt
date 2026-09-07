// The shared finalized state (issue #88): a `FinalizedPayroll` is immutable
// history, and every screen that shows one says so the same way — a lock
// icon beside the sentence, never a colour swatch alone.

import { Lock } from 'lucide-react';
import type { ReactNode } from 'react';

export function FinalizedBanner({ children }: { children: ReactNode }) {
  return (
    <p className="flex items-center gap-2 rounded-md border border-success/30 bg-success/10 px-3 py-2 text-sm font-medium text-success">
      <Lock className="size-4 shrink-0" aria-hidden="true" />
      {children}
    </p>
  );
}
