// `/app/employers/:employerId/payroll/:runId` (issue #64, parent #59 Spec 3
// of 3, §0.29/§0.31). Every member one payroll run proposes to pay, by name,
// with their current Earning lines and why they cannot be paid yet, if at
// all.
//
// `blockers` is read straight off this screen's own `GET` (`usePayrollRun`)
// on every render, including a reload — never cached separately, so a fact
// recorded on the Employment screen and this screen returned to always
// agree (§0.31, issue #64's own Deep Instructions). This screen computes no
// readiness of its own: an empty `blockers` list is the server's own answer
// that a member is ready, and nothing else here decides that.

import { Link, useParams } from 'react-router-dom';
import { ApiError } from '../api/client';
import { requestIdOf } from '../api/refusal';
import type { EarningKind, PayrollRunMemberDto } from '../api/types';
import { useEmployerId } from '../employments/useEmployments';
import { formatCents } from '../money';
import { usePayrollRun } from '../payrollRuns/usePayrollRuns';
import { NotFound } from './NotFound';
import { employmentPath } from './paths';
import { blockerSection, blockerSentence } from './payroll/blockerText';

function runWasNotFound(caught: unknown): boolean {
  return (
    caught instanceof ApiError &&
    ['not_found', 'employer_not_found', 'payroll_run_not_found'].includes(caught.code)
  );
}

function loadFailureMessage(caught: unknown): string {
  if (caught instanceof ApiError && caught.code === 'internal_error') {
    const requestId = requestIdOf(caught.details);
    if (requestId !== null) {
      return `We could not load this payroll run. Try again, and quote reference ${requestId} if the problem continues.`;
    }
  }
  return 'We could not load this payroll run.';
}

function earningKindLabel(kind: EarningKind): string {
  return kind === 'basicPay' ? 'Basic pay' : 'Taxable allowance';
}

function Member({ member, employerId }: { member: PayrollRunMemberDto; employerId: string }) {
  return (
    <li>
      <h4>{member.fullName}</h4>

      {member.earnings.length === 0 ? (
        <p>No earnings recorded.</p>
      ) : (
        <ul>
          {member.earnings.map((earning, index) => (
            // No id of its own on the wire — an Earning line is only ever
            // replaced as a whole list (`PUT .../earnings`), never addressed
            // one at a time, so its position is the only stable key here.
            <li key={index}>
              {earningKindLabel(earning.kind)}: {formatCents(earning.amountCents)}
            </li>
          ))}
        </ul>
      )}

      {/* The two "present" blockers matter as much as the two "unknown"
          ones, and an empty list is the only thing that reads as ready
          (issue #64's own Deep Instructions) — so this renders every entry
          `blockers` carries and nothing is inferred from their absence. */}
      {member.blockers.length === 0 ? (
        <p>Ready to pay.</p>
      ) : (
        <ul>
          {member.blockers.map((blocker) => (
            <li key={blocker.code} role="alert">
              {blockerSentence(blocker)}{' '}
              <Link
                to={`${employmentPath(employerId, member.employmentId)}#${blockerSection(blocker)}`}
              >
                Fix on the Employment screen
              </Link>
            </li>
          ))}
        </ul>
      )}
    </li>
  );
}

export function PayrollRun() {
  const { runId } = useParams();
  if (runId === undefined) {
    throw new Error('PayrollRun must be rendered at a route carrying :runId');
  }

  const employerId = useEmployerId();
  const run = usePayrollRun(runId);

  if (run.isError && runWasNotFound(run.error)) {
    return <NotFound />;
  }

  return (
    <main>
      <p>
        <Link to=".." relative="path">
          ← Payroll
        </Link>
      </p>

      {run.isPending && <p>Loading…</p>}

      {run.isError && (
        <p role="alert">
          {loadFailureMessage(run.error)}{' '}
          <button type="button" onClick={() => void run.refetch()}>
            Try again
          </button>
        </p>
      )}

      {run.isSuccess && (
        <>
          <h2>
            {run.data.period.start} to {run.data.period.end}
          </h2>
          <p>Pay date: {run.data.payDate}</p>

          {run.data.members.length === 0 ? (
            <p>No one is proposed to be paid on this run.</p>
          ) : (
            <ul>
              {run.data.members.map((member) => (
                <Member key={member.employmentId} member={member} employerId={employerId} />
              ))}
            </ul>
          )}
        </>
      )}
    </main>
  );
}
