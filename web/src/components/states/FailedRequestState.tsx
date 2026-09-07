// The shared failed-request state (issue #88): every screen's "we could not
// load/save this" moment renders through one component, with a real retry
// action — never a fake spinner, never a request the Operator cannot repeat.
// `role="alert"` so assistive technology hears it the moment it appears,
// matching what every screen already did before this rebuild.

import { CircleAlert } from 'lucide-react';
import { Button } from '../ui/button';

export function FailedRequestState({
  message,
  onRetry,
  retrying = false,
  retryLabel = 'Try again',
}: {
  message: string;
  onRetry: () => void;
  retrying?: boolean;
  retryLabel?: string;
}) {
  return (
    <p role="alert" className="flex flex-wrap items-center gap-2 text-sm text-destructive">
      <CircleAlert className="size-4 shrink-0" aria-hidden="true" />
      <span>{message}</span>
      <Button type="button" variant="outline" size="sm" onClick={onRetry} disabled={retrying}>
        {retrying ? 'Retrying…' : retryLabel}
      </Button>
    </p>
  );
}
