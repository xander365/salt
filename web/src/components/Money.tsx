// The one place a figure becomes `N$1,234.56` on screen (DESIGN.md's money
// convention) — every table, worksheet and figure list renders through this
// rather than calling `moneyDisplayText` and the `.money` class separately.

import { moneyDisplayText } from '../money';
import { cn } from '../lib/utils';

export function Money({ cents, className }: { cents: number; className?: string }) {
  return <span className={cn('money', className)}>{moneyDisplayText(cents)}</span>;
}
