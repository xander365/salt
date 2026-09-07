// `/app/employers/:employerId/people` (issue #62, parent #59 Spec 3 of 3,
// §0.38; rebuilt for issue #88). Every Employment this Employer has, named
// with the person's `fullName`, and a form that adds one by typing a full
// name. There is no `personId` input anywhere — a Person exists only as a
// side effect of creating an Employment, and there is no Person screen or
// Person list route to send one to.
//
// The list is what `GET /api/employers/{e}/employments` returns, re-read
// after a successful add (`useCreateEmployment` invalidates it) rather than
// grown locally — the server's list is the truth.

import { type SubmitEvent, useRef, useState } from 'react';
import { Link } from 'react-router-dom';
import { UserPlus } from 'lucide-react';
import { ApiError } from '../api/client';
import { requestIdOf } from '../api/refusal';
import { useCreateEmployment, useEmployments } from '../employments/useEmployments';
import { Button } from '../components/ui/button';
import { Input } from '../components/ui/input';
import { Label } from '../components/ui/label';
import { Card, CardContent, CardHeader, CardTitle } from '../components/ui/card';
import { LoadingState } from '../components/states/LoadingState';
import { EmptyState } from '../components/states/EmptyState';
import { FailedRequestState } from '../components/states/FailedRequestState';
import { ValidationError } from '../components/states/ValidationError';

/**
 * The Operator's own words for a refused add, chosen by `error.code` and
 * never by `error.message` (§0.23) — the codes are the contract, the
 * messages may be reworded at any time.
 *
 * Only the refusals this form can actually provoke get their own sentence.
 * `malformed_request` is not among them: it answers "both `personId` and
 * `fullName`, or neither", and this form sends exactly one of them, always
 * `fullName`.
 */
function messageForRefusal(caught: unknown): string {
  if (!(caught instanceof ApiError)) {
    return 'Something went wrong. Please try again.';
  }

  switch (caught.code) {
    case 'person_full_name_cannot_be_empty':
      return 'Enter a full name.';

    // The Employer went out of this Operator's reach between loading the
    // screen and submitting it — a membership removed, or the Employer
    // itself gone. Reloading is what shows them where they now stand.
    case 'not_found':
    case 'employer_not_found':
      return 'This employer is no longer available to you. Reload the page.';

    case 'forbidden':
      return 'Your role does not allow adding employees.';

    // §0's story 50: an unexpected failure hands back a reference the
    // Operator can quote, which a 500 carries in `details.requestId` and no
    // other status carries at all.
    case 'internal_error': {
      const requestId = requestIdOf(caught.details);
      return requestId === null
        ? 'Something went wrong. Please try again.'
        : `Something went wrong. Please try again, and quote reference ${requestId}.`;
    }

    default:
      return 'Something went wrong. Please try again.';
  }
}

export function People() {
  const employments = useEmployments();
  const createEmployment = useCreateEmployment();
  const [fullName, setFullName] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [added, setAdded] = useState<string | null>(null);
  const fullNameInput = useRef<HTMLInputElement>(null);

  async function handleSubmit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();

    // The submit button is disabled in flight, which stops the pointer and
    // stops implicit submission from the field. This guards the case neither
    // covers: a second submit raised while the first is still open.
    if (createEmployment.isPending) {
      return;
    }

    setError(null);
    setAdded(null);

    try {
      await createEmployment.mutateAsync(fullName);
      setAdded(fullName.trim());
      setFullName('');
      // Keyboard alone (this issue's own acceptance criterion): the next
      // name is typed from where the last one was, without reaching for a
      // pointer to put the caret back.
      fullNameInput.current?.focus();
    } catch (caught) {
      setError(messageForRefusal(caught));
    }
  }

  return (
    <main className="flex flex-col gap-6">
      <h2 className="text-2xl font-semibold tracking-tight">People</h2>

      {employments.isPending && <LoadingState label="Loading people…" />}

      {employments.isError && (
        <FailedRequestState
          message="We could not load the people list."
          onRetry={() => void employments.refetch()}
          retrying={employments.isFetching}
        />
      )}

      {employments.isSuccess &&
        (employments.data.employments.length === 0 ? (
          <EmptyState>No one is employed here yet. Add the first person below.</EmptyState>
        ) : (
          <ul className="flex flex-col divide-y rounded-lg border">
            {employments.data.employments.map((employment) => (
              <li key={employment.employmentId}>
                {/* Relative, not `employmentPath` rebuilt from `:employerId`:
                    this screen already renders beneath
                    `/app/employers/:employerId/people` (issue #62's own
                    Deep Instructions, followed here). */}
                <Link
                  to={employment.employmentId}
                  className="block px-4 py-3 text-sm font-medium hover:bg-accent hover:text-accent-foreground"
                >
                  {employment.fullName}
                </Link>
              </li>
            ))}
          </ul>
        ))}

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base">
            <UserPlus className="size-4" aria-hidden="true" />
            Add a person
          </CardTitle>
        </CardHeader>
        <CardContent>
          <form onSubmit={handleSubmit} className="flex flex-col gap-4 sm:max-w-sm">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="fullName">Full name</Label>
              <Input
                id="fullName"
                name="fullName"
                type="text"
                // These are other people's names, never the Operator's own,
                // so there is nothing here a browser's autofill knows
                // better.
                autoComplete="off"
                required
                ref={fullNameInput}
                aria-invalid={error !== null}
                aria-describedby={error === null ? undefined : 'fullNameError'}
                value={fullName}
                onChange={(event) => {
                  setFullName(event.target.value);
                  // The refusal was about the value that has just changed,
                  // so leaving it on screen would keep answering a stale
                  // question.
                  setError(null);
                }}
              />
            </div>
            {error !== null && <ValidationError id="fullNameError">{error}</ValidationError>}
            <Button type="submit" disabled={createEmployment.isPending} className="sm:self-start">
              {createEmployment.isPending ? 'Adding…' : 'Add person'}
            </Button>
          </form>

          {/* The list row is the confirmation for anyone who can see it.
              This says the same thing out loud, because a caret that stayed
              in an emptied field is not, on its own, news that the add
              took. */}
          <p role="status" className="mt-3 text-sm text-success">
            {added === null ? '' : `Added ${added}.`}
          </p>
        </CardContent>
      </Card>
    </main>
  );
}
