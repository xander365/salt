// The shared blocked state (issue #88): one standing reason a payroll run
// member cannot be paid yet, with the link to its remedy — replacing a plain
// `<li>` with an icon and a consistent card so a blocker reads as "stop and
// fix this" rather than ordinary list content.

import type { ReactNode } from 'react';
import { Link } from 'react-router-dom';
import { OctagonAlert } from 'lucide-react';

export function BlockedItem({
  to,
  linkLabel = 'Fix on the Employment screen',
  children,
}: {
  to: string;
  linkLabel?: string;
  children: ReactNode;
}) {
  return (
    <li className="flex items-start gap-2 rounded-md border border-warning/30 bg-warning/10 px-3 py-2 text-sm">
      <OctagonAlert className="mt-0.5 size-4 shrink-0 text-warning" aria-hidden="true" />
      <span>
        {children} <Link to={to}>{linkLabel}</Link>
      </span>
    </li>
  );
}
