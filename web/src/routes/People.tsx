// `/app/employers/:employerId/people` (issue #62, parent #59 Spec 3 of 3,
// §0.38). Every Employment this Employer has, named with the person's
// `fullName`, and a form that adds one by typing a full name. There is no
// `personId` input anywhere — a Person exists only as a side effect of
// creating an Employment, and there is no Person screen or Person list route
// to send one to.
//
// The list is what `GET /api/employers/{e}/employments` returns, re-read
// after a successful add (`useCreateEmployment` invalidates it) rather than
// grown locally — the server's list is the truth.

import { type SubmitEvent, useRef, useState } from 'react';
import { Link } from 'react-router-dom';
import { ApiError } from '../api/client';
import { useCreateEmployment, useEmployments } from '../employments/useEmployments';

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

function requestIdOf(details: unknown): string | null {
  if (typeof details !== 'object' || details === null) {
    return null;
  }
  const requestId = (details as { requestId?: unknown }).requestId;
  return typeof requestId === 'string' && requestId !== '' ? requestId : null;
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
    <main>
      <h2>People</h2>

      {employments.isPending && <p>Loading…</p>}

      {employments.isError && (
        <p role="alert">
          We could not load the people list.{' '}
          <button type="button" onClick={() => void employments.refetch()}>
            Try again
          </button>
        </p>
      )}

      {employments.isSuccess &&
        (employments.data.employments.length === 0 ? (
          <p>No one is employed here yet.</p>
        ) : (
          <ul>
            {employments.data.employments.map((employment) => (
              <li key={employment.employmentId}>
                {/* Relative, not `employmentPath` rebuilt from `:employerId`:
                    this screen already renders beneath
                    `/app/employers/:employerId/people` (issue #62's own
                    Deep Instructions, followed here). */}
                <Link to={employment.employmentId}>{employment.fullName}</Link>
              </li>
            ))}
          </ul>
        ))}

      <h3>Add an employee</h3>
      <form onSubmit={handleSubmit}>
        <div>
          <label htmlFor="fullName">Full name</label>
          <input
            id="fullName"
            name="fullName"
            type="text"
            // These are other people's names, never the Operator's own, so
            // there is nothing here a browser's autofill knows better.
            autoComplete="off"
            required
            ref={fullNameInput}
            aria-invalid={error !== null}
            aria-describedby={error === null ? undefined : 'fullNameError'}
            value={fullName}
            onChange={(event) => {
              setFullName(event.target.value);
              // The refusal was about the value that has just changed, so
              // leaving it on screen would keep answering a stale question.
              setError(null);
            }}
          />
        </div>
        {error !== null && (
          <p id="fullNameError" role="alert">
            {error}
          </p>
        )}
        <button type="submit" disabled={createEmployment.isPending}>
          {createEmployment.isPending ? 'Adding…' : 'Add employee'}
        </button>
      </form>

      {/* The list row is the confirmation for anyone who can see it. This
          says the same thing out loud, because a caret that stayed in an
          emptied field is not, on its own, news that the add took. */}
      <p role="status">{added === null ? '' : `Added ${added}.`}</p>
    </main>
  );
}
