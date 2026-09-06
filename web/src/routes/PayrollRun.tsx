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

import { useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import { ApiError } from '../api/client';
import { requestIdOf } from '../api/refusal';
import type {
  PayrollRunBlockerDto,
  PayrollRunDetailResponse,
  PayrollRunMemberDto,
} from '../api/types';
import { useEmployerId } from '../employments/useEmployments';
import {
  useCalculatePayrollRun,
  useFinalizePayrollRun,
  usePayrollRun,
} from '../payrollRuns/usePayrollRuns';
import { NotFound } from './NotFound';
import { employmentPath, finalizedPayrollPath } from './paths';
import { blockerSection, blockerSentence } from './payroll/blockerText';
import { EarningsForm } from './payroll/EarningsForm';
import {
  alreadyFinalizedDetailsOf,
  finalizeFailureMessage,
  isFinalizationMismatch,
} from './payroll/finalizeText';
import { Figures } from './payroll/Figures';
import { refusalSentence } from './payroll/refusalText';

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

function calculateFailureMessage(caught: unknown): string {
  if (!(caught instanceof ApiError)) {
    return 'Something went wrong. Please try again.';
  }
  switch (caught.code) {
    case 'payroll_run_already_finalized':
      return 'This payroll run has already been finalized.';
    case 'internal_error': {
      const requestId = requestIdOf(caught.details);
      return requestId === null
        ? 'We could not calculate this payroll run. Please try again.'
        : `We could not calculate this payroll run. Try again, and quote reference ${requestId} if the problem continues.`;
    }
    default:
      return 'We could not calculate this payroll run. Please try again.';
  }
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

function Member({
  member,
  employerId,
  payrollRunId,
}: {
  member: PayrollRunMemberDto;
  employerId: string;
  payrollRunId: string;
}) {
  return (
    <li>
      <h3>{member.fullName}</h3>

      <EarningsForm
        payrollRunId={payrollRunId}
        employmentId={member.employmentId}
        earnings={member.earnings}
      />

      {/* The two "present" blockers matter as much as the two "unknown"
          ones. An empty list says only that no standing fact is currently
          blocking calculation — a fresh calculation may still refuse for a
          calculation-time reason. */}
      {member.blockers.length === 0 ? (
        <p>No standing blockers.</p>
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

      {/* `figures` and `refusal` are different things and both may be
          present (§0.31, issue #65's own Deep Instructions): the first is
          this member's current calculation, however it got there; the
          second is only ever what the very last Calculate said, and is
          never on a plain reload. Each renders from its own field, and
          neither is inferred from the other.

          A member with no `figures` says so plainly. After a plain reload
          its `refusal` is gone even though the reason it refused is not,
          and an empty `blockers` list beside no figures would otherwise
          read as a member with nothing wrong at all (§0.31). */}
      {member.figures === null ? (
        <p>No figures yet. Calculate this run to see them.</p>
      ) : (
        <Figures figures={member.figures} />
      )}

      {/* `role="alert"`, unlike a blocker: this is what the Calculate an
          Operator just ran said about this member, not standing content
          already on the screen when it loaded. */}
      {member.refusal !== null && (
        <p role="alert">Could not calculate: {refusalSentence(member.refusal)}</p>
      )}
    </li>
  );
}

/**
 * `POST .../payroll-runs/{r}/finalize` (issue #66): a plain confirmation
 * naming how many people the run covers — no password re-entry, no typed
 * word, no second approver (issue #66's own acceptance criteria) — then the
 * one atomic act that turns a Calculated run into immutable history.
 *
 * Offered only while `run.status === 'calculated'` (§0's Deep Instructions:
 * the browser reads readiness off the server's own status, never decides it
 * itself) — except while `finalizedLinks` is set, which outlives that status
 * changing underneath it once the success or the already-finalized response
 * has already been read.
 *
 * A successful call with exactly one finalized member navigates straight to
 * it: the tracer-bullet case, and the only case §0.28's own retry shortcut
 * ever names directly. More than one member has no single "the" finalized
 * payroll to land on — `crates/salt-server/src/payroll_error.rs`'s own
 * comment calls this "the case with one answer" — so this screen stays put
 * and lists a link to each member's own finalized payroll instead of
 * guessing which one to show.
 */
function Finalize({
  run,
  employerId,
  payrollRunId,
  onCalculateAgain,
  calculateIsPending,
}: {
  run: PayrollRunDetailResponse;
  employerId: string;
  payrollRunId: string;
  onCalculateAgain: () => void;
  calculateIsPending: boolean;
}) {
  const navigate = useNavigate();
  const finalize = useFinalizePayrollRun(payrollRunId);
  const [confirming, setConfirming] = useState(false);
  const [finalizedLinks, setFinalizedLinks] = useState<
    { employmentId: string; finalizedPayrollId: string }[] | null
  >(null);

  async function handleFinalize() {
    try {
      const result = await finalize.mutateAsync();
      setConfirming(false);
      if (result.finalized.length === 1) {
        navigate(finalizedPayrollPath(employerId, result.finalized[0].finalizedPayrollId));
      } else {
        setFinalizedLinks(result.finalized);
      }
    } catch (caught) {
      // §0.28: a retried finalize that actually landed reads back this code
      // and this screen navigates to what already succeeded — never a red
      // banner for a request that worked. Every other refusal is read from
      // `finalize.isError`/`finalize.error` below.
      if (caught instanceof ApiError && caught.code === 'payroll_run_already_finalized') {
        const { finalizedPayrollId, finalizedPayrolls } = alreadyFinalizedDetailsOf(caught.details);
        setConfirming(false);
        if (finalizedPayrollId !== null) {
          navigate(finalizedPayrollPath(employerId, finalizedPayrollId));
        } else {
          setFinalizedLinks(finalizedPayrolls);
        }
      }
    }
  }

  if (finalizedLinks !== null) {
    return (
      <div role="status">
        <p>Finalized. Open each person’s finalized payroll:</p>
        <ul>
          {finalizedLinks.map((entry) => {
            const member = run.members.find(
              (candidate) => candidate.employmentId === entry.employmentId,
            );
            return (
              <li key={entry.employmentId}>
                <Link to={finalizedPayrollPath(employerId, entry.finalizedPayrollId)}>
                  {member?.fullName ?? entry.employmentId}
                </Link>
              </li>
            );
          })}
        </ul>
      </div>
    );
  }

  if (run.status !== 'calculated') {
    return null;
  }

  const failureMessage = finalize.isError
    ? finalizeFailureMessage(finalize.error, run.members)
    : null;

  return (
    <div>
      {confirming ? (
        <p>
          This creates immutable payroll history for {run.members.length}{' '}
          {run.members.length === 1 ? 'person' : 'people'}.{' '}
          <button type="button" onClick={() => void handleFinalize()} disabled={finalize.isPending}>
            {finalize.isPending ? 'Finalizing…' : 'Confirm finalize'}
          </button>{' '}
          <button type="button" onClick={() => setConfirming(false)} disabled={finalize.isPending}>
            Cancel
          </button>
        </p>
      ) : (
        <p>
          <button type="button" onClick={() => setConfirming(true)}>
            Finalize
          </button>
        </p>
      )}

      {failureMessage !== null && (
        <p role="alert">
          {failureMessage}{' '}
          {isFinalizationMismatch(finalize.error) && (
            <button type="button" onClick={onCalculateAgain} disabled={calculateIsPending}>
              Calculate again
            </button>
          )}
        </p>
      )}
    </div>
  );
}

export function PayrollRun() {
  const { runId } = useParams();
  if (runId === undefined) {
    throw new Error('PayrollRun must be rendered at a route carrying :runId');
  }

  const employerId = useEmployerId();
  const run = usePayrollRun(runId);
  const calculate = useCalculatePayrollRun(runId);

  if (run.isError && runWasNotFound(run.error)) {
    return <NotFound />;
  }

  async function handleCalculate() {
    try {
      await calculate.mutateAsync();
    } catch {
      // `calculate.isError` and `calculate.error` already carry this for
      // the render below — nothing further to do here.
    }
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
          <p>Status: {run.data.status}</p>

          {/* Finalize belongs to issue #66, and is offered there only when
              `status` itself says so (§0's Deep Instructions: the browser
              never decides that). Calculate has no such gate of its own —
              it is always safe to run again, on a Draft or a Calculated run
              alike — except once history is written, and a Finalized run's
              own Calculate call would refuse that itself if ever clicked
              from a stale screen. */}
          {run.data.status !== 'finalized' && (
            <p>
              <button
                type="button"
                onClick={() => void handleCalculate()}
                disabled={calculate.isPending}
              >
                {calculate.isPending ? 'Calculating…' : 'Calculate'}
              </button>
            </p>
          )}

          {calculate.isError && <p role="alert">{calculateFailureMessage(calculate.error)}</p>}

          <Finalize
            run={run.data}
            employerId={employerId}
            payrollRunId={runId}
            onCalculateAgain={() => void handleCalculate()}
            calculateIsPending={calculate.isPending}
          />

          {run.data.members.length === 0 ? (
            <p>No one is proposed to be paid on this run.</p>
          ) : (
            <ul>
              {run.data.members.map((member) => (
                <Member
                  key={member.employmentId}
                  member={member}
                  employerId={employerId}
                  payrollRunId={runId}
                />
              ))}
            </ul>
          )}
        </>
      )}
    </main>
  );
}
