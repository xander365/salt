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

import { type SubmitEvent, useState } from 'react';
import { ApiError } from '../api/client';
import { useCreateEmployment, useEmployments } from '../employments/useEmployments';

export function People() {
  const employments = useEmployments();
  const createEmployment = useCreateEmployment();
  const [fullName, setFullName] = useState('');
  const [error, setError] = useState<string | null>(null);

  async function handleSubmit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);

    try {
      await createEmployment.mutateAsync(fullName);
      setFullName('');
    } catch (caught) {
      if (caught instanceof ApiError && caught.code === 'person_full_name_cannot_be_empty') {
        setError('Enter a full name.');
      } else {
        setError('Something went wrong. Please try again.');
      }
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
              <li key={employment.employmentId}>{employment.fullName}</li>
            ))}
          </ul>
        ))}

      <form onSubmit={handleSubmit}>
        <div>
          <label htmlFor="fullName">Full name</label>
          <input
            id="fullName"
            name="fullName"
            type="text"
            required
            value={fullName}
            onChange={(event) => setFullName(event.target.value)}
          />
        </div>
        {error !== null && <p role="alert">{error}</p>}
        <button type="submit" disabled={createEmployment.isPending}>
          {createEmployment.isPending ? 'Adding…' : 'Add employee'}
        </button>
      </form>
    </main>
  );
}
