// The shared inline validation state (issue #88): a field-level complaint
// pairs an icon with the text, so the error is never colour alone — and
// keeps the `id`/`role="alert"` contract every form's `fieldErrorProps`
// already points `aria-describedby` at.

import type { ReactNode } from 'react';
import { CircleAlert } from 'lucide-react';

export function ValidationError({ id, children }: { id?: string; children: ReactNode }) {
  return (
    <p id={id} role="alert" className="mt-1.5 flex items-start gap-1.5 text-sm text-destructive">
      <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
      <span>{children}</span>
    </p>
  );
}
