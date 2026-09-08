// The Employer's own landing screen, inside the shell that names it (issue
// #88: the "Employer" third of D13's People / Payroll / Employer
// navigation). `people` and `payroll` are sibling routes reached from the
// persistent nav above (issues #62 and #64) — this screen adds no second,
// differently-worded way to reach either: two links named alike would leave
// an Operator (and a role/name-driven test) unable to tell them apart.
//
// Issue #71 gives this screen its first real content: `EmployerParticulars`,
// the registered name, address and statutory registration numbers a payslip
// must carry. Recording and correcting them is Owner-only (§0.6) — a
// PayrollOperator still reads this screen (their membership already proves
// that much), but `EmployerParticularsForm` renders their own details as
// plain text instead of a form neither of them can submit.

import { useParams } from 'react-router-dom';
import { ApiError } from '../api/client';
import { requestIdOf } from '../api/refusal';
import { useAuthorizedSession } from '../session/AuthorizedSession';
import { useEmployerParticulars } from '../employer/useEmployerParticulars';
import { EmployerParticularsForm } from './employer/EmployerParticularsForm';
import { LoadingState } from '../components/states/LoadingState';
import { FailedRequestState } from '../components/states/FailedRequestState';

function particularsLoadFailureMessage(caught: unknown): string {
  if (caught instanceof ApiError && caught.code === 'internal_error') {
    const requestId = requestIdOf(caught.details);
    if (requestId !== null) {
      return `We could not load this Employer’s particulars. Try again, and quote reference ${requestId} if the problem continues.`;
    }
  }
  return 'We could not load this Employer’s particulars.';
}

export function EmployerHome() {
  const session = useAuthorizedSession();
  const { employerId } = useParams();
  const particulars = useEmployerParticulars();

  const membership = session.memberships.find((candidate) => candidate.employerId === employerId);
  const canEdit = membership?.role === 'owner';

  return (
    <main className="flex flex-col gap-6">
      <div>
        <p className="text-sm text-muted-foreground">
          Signed in as {session.operator.displayName}.
        </p>
        <p className="text-muted-foreground">
          Use People to add people and record what they are paid, or Payroll to run, calculate and
          finalize a pay period.
        </p>
      </div>

      {particulars.isPending && <LoadingState label="Loading employer particulars…" />}

      {particulars.isError && (
        <FailedRequestState
          message={particularsLoadFailureMessage(particulars.error)}
          onRetry={() => void particulars.refetch()}
          retrying={particulars.isFetching}
        />
      )}

      {particulars.isSuccess && (
        <EmployerParticularsForm particulars={particulars.data} canEdit={canEdit} />
      )}
    </main>
  );
}
