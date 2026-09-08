// `/app/employers/:employerId/finalized/:finalizedPayrollId` (issue #66,
// parent #59 Spec 3 of 3, §0.29; rebuilt for issue #88). One immutable
// finalized payroll: the nine figures, the period, the pay date and the
// SaltVersion that produced them.
//
// There is no lifecycle to read here and nothing this screen could offer to
// change (§2.5: finalization is the approval, and there is no `Reviewed`
// state). Signing out and back in, or reloading, reads the same figures
// every time, because a `FinalizedPayroll` never changes once written.

import { ArrowLeft } from 'lucide-react';
import { Link, useParams } from 'react-router-dom';
import { ApiError } from '../api/client';
import { requestIdOf } from '../api/refusal';
import { useFinalizedPayroll } from '../finalizedPayroll/useFinalizedPayroll';
import { FrozenParticulars } from '../finalizedPayroll/FrozenParticulars';
import { Workings } from '../finalizedPayroll/Workings';
import { useEmployerId } from '../employments/useEmployments';
import { humanDate, humanDateRange } from '../format';
import { NotFound } from './NotFound';
import { employerPath } from './paths';
import { Figures } from './payroll/Figures';
import { LoadingState } from '../components/states/LoadingState';
import { FailedRequestState } from '../components/states/FailedRequestState';
import { FinalizedBanner } from '../components/states/FinalizedBanner';

function finalizedPayrollWasNotFound(caught: unknown): boolean {
  return (
    caught instanceof ApiError &&
    ['not_found', 'employer_not_found', 'finalized_payroll_not_found'].includes(caught.code)
  );
}

function loadFailureMessage(caught: unknown): string {
  if (caught instanceof ApiError && caught.code === 'internal_error') {
    const requestId = requestIdOf(caught.details);
    if (requestId !== null) {
      return `We could not load this finalized payroll. Try again, and quote reference ${requestId} if the problem continues.`;
    }
  }
  return 'We could not load this finalized payroll.';
}

export function FinalizedPayroll() {
  const { finalizedPayrollId } = useParams();
  if (finalizedPayrollId === undefined) {
    throw new Error('FinalizedPayroll must be rendered at a route carrying :finalizedPayrollId');
  }

  const employerId = useEmployerId();
  const finalizedPayroll = useFinalizedPayroll(finalizedPayrollId);

  if (finalizedPayroll.isError && finalizedPayrollWasNotFound(finalizedPayroll.error)) {
    return <NotFound />;
  }

  return (
    <main className="flex flex-col gap-6">
      {/* Named distinctly from the persistent "Payroll" nav link beside it,
          the same reason `PayrollRun.tsx`'s own back link is. */}
      <Link
        to={`${employerPath(employerId)}/payroll`}
        aria-label="Back to Payroll"
        className="flex w-fit items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
      >
        <ArrowLeft className="size-4" aria-hidden="true" />
        Payroll
      </Link>

      {finalizedPayroll.isPending && <LoadingState label="Loading finalized payroll…" />}

      {finalizedPayroll.isError && (
        <FailedRequestState
          message={loadFailureMessage(finalizedPayroll.error)}
          onRetry={() => void finalizedPayroll.refetch()}
          retrying={finalizedPayroll.isFetching}
        />
      )}

      {finalizedPayroll.isSuccess && (
        <>
          <div>
            <h2 className="text-2xl font-semibold tracking-tight">
              {finalizedPayroll.data.fullName}:{' '}
              {humanDateRange(finalizedPayroll.data.period.start, finalizedPayroll.data.period.end)}
            </h2>
            <p className="text-sm text-muted-foreground">
              Pay date: {humanDate(finalizedPayroll.data.payDate)}
            </p>
          </div>
          <FinalizedBanner>This payroll is finalized and cannot be changed.</FinalizedBanner>
          <Figures figures={finalizedPayroll.data.figures} />
          {/* Issue #73: the Employer and Person particulars frozen at
              finalize time, never a later correction of either (D22). */}
          <FrozenParticulars
            employerParticulars={finalizedPayroll.data.employerParticulars}
            personParticulars={finalizedPayroll.data.personParticulars}
          />
          <p className="text-sm text-muted-foreground">
            Salt version: {finalizedPayroll.data.saltVersion}
          </p>
          {/* Last, and closed: the everyday facts above read exactly as they
              did before issue #67, and the workings are there when asked
              for (§0.29). */}
          <Workings finalizedPayrollId={finalizedPayrollId} />
        </>
      )}
    </main>
  );
}
