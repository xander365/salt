// `/app/employers/:employerId/people/:employmentId` (issue #63, parent #59
// Spec 3 of 3, §0.35; rebuilt for issue #88). An Operator opens one
// Employment and records everything it needs to become payable: what it is
// paid and from when, whether the employee had earlier taxable employment
// this tax year, whether they have deductions Salt does not support, and an
// opening balance when Salt was adopted mid-year.
//
// Each of the four forms below owns its own submission, its own refusal
// message and its own saved-confirmation text — none of this screen's state
// is shared between them, because a refusal or a save in one has nothing to
// say about the others.

import { ArrowLeft } from 'lucide-react';
import { Link, useParams } from 'react-router-dom';
import { ApiError } from '../api/client';
import { requestIdOf } from '../api/refusal';
import { useEmployment } from '../employments/useEmployments';
import { moneyDisplayText } from '../money';
import { humanDate } from '../format';
import { CompensationTermsForm } from './employment/CompensationTermsForm';
import { OpeningBalanceForm } from './employment/OpeningBalanceForm';
import { PriorEmploymentForm } from './employment/PriorEmploymentForm';
import { UnsupportedDeductionStatusForm } from './employment/UnsupportedDeductionStatusForm';
import { useScrollToSection } from './employment/useScrollToSection';
import { NotFound } from './NotFound';
import { LoadingState } from '../components/states/LoadingState';
import { FailedRequestState } from '../components/states/FailedRequestState';

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
  return moneyDisplayText(cents);
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
    <main className="flex flex-col gap-6">
      {/* Named distinctly from the persistent "People" nav link beside it
          (both are on screen at once): two links sharing one accessible
          name on the same page is an ambiguity a screen reader's link list
          would surface immediately, even though nothing here happens to
          collide with it. */}
      <Link
        to=".."
        relative="path"
        aria-label="Back to People"
        className="flex w-fit items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
      >
        <ArrowLeft className="size-4" aria-hidden="true" />
        People
      </Link>

      {employment.isPending && <LoadingState label="Loading employment…" />}

      {employment.isError && (
        <FailedRequestState
          message={employmentLoadFailureMessage(employment.error)}
          onRetry={() => void employment.refetch()}
          retrying={employment.isFetching}
        />
      )}

      {employment.isSuccess && (
        <>
          <div>
            <h2 className="text-2xl font-semibold tracking-tight">{employment.data.fullName}</h2>
            <p className="text-sm text-muted-foreground">
              Employed from {humanDate(employment.data.startDate)}
              {employment.data.endDate === null ? '' : ` to ${humanDate(employment.data.endDate)}`}.
              Current pay: {currentPayText(employment.data.currentBasicPayCents)}
            </p>
          </div>

          <div className="flex flex-col gap-6">
            <CompensationTermsForm employmentId={employmentId} />
            <PriorEmploymentForm employmentId={employmentId} />
            <UnsupportedDeductionStatusForm employmentId={employmentId} />
            <OpeningBalanceForm employmentId={employmentId} />
          </div>
        </>
      )}
    </main>
  );
}
