// The part of `messageForRefusal` every screen under an Employment needs
// alike (People.tsx's own pattern, issue #62): the handful of refusals that
// are about reaching the screen at all, never about one particular form's
// fields. Branches on `error.code` only (§0.23) — `message` may be reworded
// at any time.

import { ApiError } from './client';

export function requestIdOf(details: unknown): string | null {
  if (typeof details !== 'object' || details === null) {
    return null;
  }
  const requestId = (details as { requestId?: unknown }).requestId;
  return typeof requestId === 'string' && requestId !== '' ? requestId : null;
}

/**
 * `null` when `caught` is not one of the shared codes — the caller's own
 * `switch` then decides, and finally falls back to a generic retry message.
 */
export function sharedFactRefusalMessage(caught: unknown): string | null {
  if (!(caught instanceof ApiError)) {
    return 'Something went wrong. Please try again.';
  }

  switch (caught.code) {
    // The Employment (or its Employer) went out of this Operator's reach
    // between loading the screen and submitting a form.
    case 'not_found':
    case 'employer_not_found':
    case 'employment_not_found':
    case 'person_not_found':
      return 'This employee is no longer available to you. Reload the page.';

    case 'employment_is_void':
      return 'This employment has been voided and can no longer be changed.';

    case 'forbidden':
      return 'Your role does not allow this.';

    // §0's story 50: an unexpected failure hands back a reference the
    // Operator can quote, carried in `details.requestId` on a 500 only.
    case 'internal_error': {
      const requestId = requestIdOf(caught.details);
      return requestId === null
        ? 'Something went wrong. Please try again.'
        : `Something went wrong. Please try again, and quote reference ${requestId}.`;
    }

    default:
      return null;
  }
}
