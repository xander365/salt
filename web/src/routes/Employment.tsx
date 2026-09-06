// `/app/employers/:employerId/people/:employmentId` (issue #63, parent #59
// Spec 3 of 3, §0.35). An Operator opens one Employment and records
// everything it needs to become payable: what it is paid and from when,
// whether the employee had earlier taxable employment this tax year,
// whether they have deductions Salt does not support, and an opening
// balance when Salt was adopted mid-year.
//
// Each of the four forms below owns its own submission, its own refusal
// message and its own saved-confirmation text — none of this screen's state
// is shared between them, because a refusal or a save in one has nothing to
// say about the others.

import { Link, useParams } from 'react-router-dom';
import { ApiError } from '../api/client';
import { requestIdOf } from '../api/refusal';
import { useEmployment } from '../employments/useEmployments';
import { formatCents } from '../money';
import { CompensationTermsForm } from './employment/CompensationTermsForm';
import { OpeningBalanceForm } from './employment/OpeningBalanceForm';
import { PriorEmploymentForm } from './employment/PriorEmploymentForm';
import { UnsupportedDeductionStatusForm } from './employment/UnsupportedDeductionStatusForm';
import { useScrollToSection } from './employment/useScrollToSection';
import { NotFound } from './NotFound';

function employmentWasNotFound(caught: unknown): boolean {
  return (
    caught instanceof ApiError &&
    ['not_found', 'employer_not_found', 'employment_not_found'].includes(caught.code)
  );
}

function employmentLoadFailureMessage(caught: unknown): string {
  if (caught instanceof ApiError && caught.code === 'internal_error') {
    const requestId = requestIdOf(caught.details);
    if (requestId !== null) {
      return `We could not load this Employment. Try again, and quote reference ${requestId} if the problem continues.`;
    }
  }
  return 'We could not load this Employment.';
}

function currentPayText(cents: number | null): string {
  if (cents === null) {
    return 'not yet recorded';
  }
  if (!Number.isSafeInteger(cents) || cents < 0) {
    return 'unavailable because the amount cannot be displayed exactly';
  }
  return formatCents(cents);
}

export function Employment() {
  const { employmentId } = useParams();
  if (employmentId === undefined) {
    throw new Error('Employment must be rendered at a route carrying :employmentId');
  }

  const employment = useEmployment(employmentId);

  // A payroll run's blocker links here at `#prior-employment` and the three
  // beside it. The sections exist only once the screen below has drawn
  // them, so the scroll waits for that (issue #64).
  useScrollToSection(employment.isSuccess);

  if (employment.isError && employmentWasNotFound(employment.error)) {
    return <NotFound />;
  }

  return (
    <main>
      <p>
        <Link to=".." relative="path">
          ← People
        </Link>
      </p>

      {employment.isPending && <p>Loading…</p>}

      {employment.isError && (
        <p role="alert">
          {employmentLoadFailureMessage(employment.error)}{' '}
          <button type="button" onClick={() => void employment.refetch()}>
            Try again
          </button>
        </p>
      )}

      {employment.isSuccess && (
        <>
          <h2>{employment.data.fullName}</h2>
          <p>
            Employed from {employment.data.startDate}
            {employment.data.endDate === null ? '' : ` to ${employment.data.endDate}`}.
          </p>
          <p>Current pay: {currentPayText(employment.data.currentBasicPayCents)}</p>

          <CompensationTermsForm employmentId={employmentId} />
          <PriorEmploymentForm employmentId={employmentId} />
          <UnsupportedDeductionStatusForm employmentId={employmentId} />
          <OpeningBalanceForm employmentId={employmentId} />
        </>
      )}
    </main>
  );
}
