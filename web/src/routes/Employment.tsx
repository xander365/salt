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
import { useEmployment } from '../employments/useEmployments';
import { formatCents } from '../money';
import { CompensationTermsForm } from './employment/CompensationTermsForm';
import { OpeningBalanceForm } from './employment/OpeningBalanceForm';
import { PriorEmploymentForm } from './employment/PriorEmploymentForm';
import { UnsupportedDeductionStatusForm } from './employment/UnsupportedDeductionStatusForm';

export function Employment() {
  const { employmentId } = useParams();
  if (employmentId === undefined) {
    throw new Error('Employment must be rendered at a route carrying :employmentId');
  }

  const employment = useEmployment(employmentId);

  return (
    <main>
      <p>
        <Link to="..">← People</Link>
      </p>

      {employment.isPending && <p>Loading…</p>}

      {employment.isError && (
        <p role="alert">
          We could not load this employee.{' '}
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
          <p>
            Current pay:{' '}
            {employment.data.currentBasicPayCents === null
              ? 'not yet recorded'
              : formatCents(employment.data.currentBasicPayCents)}
          </p>

          <CompensationTermsForm employmentId={employmentId} />
          <PriorEmploymentForm employmentId={employmentId} />
          <UnsupportedDeductionStatusForm employmentId={employmentId} />
          <OpeningBalanceForm employmentId={employmentId} />
        </>
      )}
    </main>
  );
}
