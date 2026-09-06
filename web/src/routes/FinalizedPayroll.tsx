// `/app/employers/:employerId/finalized/:finalizedPayrollId` (issue #66,
// parent #59 Spec 3 of 3, §0.29). One immutable finalized payroll: the nine
// figures, the period, the pay date and the SaltVersion that produced them.
//
// There is no lifecycle to read here and nothing this screen could offer to
// change (§2.5: finalization is the approval, and there is no `Reviewed`
// state). Signing out and back in, or reloading, reads the same figures
// every time, because a `FinalizedPayroll` never changes once written.

import { Link, useParams } from 'react-router-dom';
import { ApiError } from '../api/client';
import { requestIdOf } from '../api/refusal';
import { useFinalizedPayroll } from '../finalizedPayroll/useFinalizedPayroll';
import { useEmployerId } from '../employments/useEmployments';
import { NotFound } from './NotFound';
import { employerPath } from './paths';
import { Figures } from './payroll/Figures';

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
    <main>
      <p>
        <Link to={`${employerPath(employerId)}/payroll`}>← Payroll</Link>
      </p>

      {finalizedPayroll.isPending && <p>Loading…</p>}

      {finalizedPayroll.isError && (
        <p role="alert">
          {loadFailureMessage(finalizedPayroll.error)}{' '}
          <button type="button" onClick={() => void finalizedPayroll.refetch()}>
            Try again
          </button>
        </p>
      )}

      {finalizedPayroll.isSuccess && (
        <>
          <h2>
            {finalizedPayroll.data.period.start} to {finalizedPayroll.data.period.end}
          </h2>
          <p>Pay date: {finalizedPayroll.data.payDate}</p>
          <p>This payroll is finalized and cannot be changed.</p>
          <Figures figures={finalizedPayroll.data.figures} />
          <p>Salt version: {finalizedPayroll.data.saltVersion}</p>
        </>
      )}
    </main>
  );
}
