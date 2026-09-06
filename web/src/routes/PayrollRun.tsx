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
import type { EarningKind, PayrollRunBlockerDto, PayrollRunMemberDto } from '../api/types';
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

/**
 * `formatCents` throws rather than show an amount it cannot render exactly
 * (INV-001, `money.ts`). Thrown from here that would blank the whole run
 * over one line, hiding every other member and every other blocker — so the
 * one line says what it cannot show and the rest of the payroll still
 * reads. Same answer `Employment.tsx`'s own `currentPayText` gives.
 */
function earningAmountText(cents: number): string {
  return Number.isSafeInteger(cents) && cents >= 0
    ? formatCents(cents)
    : 'an amount that cannot be displayed exactly';
}

/** The Employment screen a blocker's fix lives on, at the section that
 * holds the form when this blocker names one. */
function blockerFixPath(
  employerId: string,
  member: PayrollRunMemberDto,
  blocker: PayrollRunBlockerDto,
): string {
  const path = employmentPath(employerId, member.employmentId);
  const section = blockerSection(blocker);
  return section === null ? path : `${path}#${section}`;
}

function Member({ member, employerId }: { member: PayrollRunMemberDto; employerId: string }) {
  return (
    <li>
      <h3>{member.fullName}</h3>

      {member.earnings.length === 0 ? (
        <p>No earnings recorded.</p>
      ) : (
        <ul>
          {member.earnings.map((earning, index) => (
            // No id of its own on the wire — an Earning line is only ever
            // replaced as a whole list (`PUT .../earnings`), never addressed
            // one at a time, so its position is the only stable key here.
            <li key={index}>
              {earningKindLabel(earning.kind)}: {earningAmountText(earning.amountCents)}
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
          {/* Not `role="alert"`: a blocker is standing content that is
              already on the screen when it loads, not something that just
              happened. Marking each one assertive would make a screen
              reader interrupt itself once per blocker on every load. */}
          {member.blockers.map((blocker) => (
            <li key={blocker.code}>
              {blockerSentence(blocker)}{' '}
              <Link to={blockerFixPath(employerId, member, blocker)}>
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

      {/* An Operator arriving back from clearing a blocker sees this run's
          last response first, while the re-read that decides whether the
          blocker is really gone is still in flight. Saying so is the honest
          version of that moment; showing an old blocker in silence is not. */}
      {run.isSuccess && run.isFetching && <p>Refreshing…</p>}

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
