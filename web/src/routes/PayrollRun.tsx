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

import { useEffect, useRef, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import { ApiError } from '../api/client';
import { requestIdOf } from '../api/refusal';
import type {
  EarningLineDto,
  PayrollRunBlockerDto,
  PayrollRunDetailResponse,
  PayrollRunMemberDto,
} from '../api/types';
import { useEmployerId } from '../employments/useEmployments';
import { centsText } from '../money';
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
  offersCalculateAgain,
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

/** A finalized run's Earnings, read back rather than edited. `PUT
 * .../earnings` refuses once history is written (`payroll_run_already_finalized`),
 * so offering the editor here would be a form whose every save is a refusal
 * — the same reason Finalize and Calculate are both gone by this point. */
function FinalizedEarnings({ earnings }: { earnings: EarningLineDto[] }) {
  if (earnings.length === 0) {
    return <p>No taxable allowances on this line.</p>;
  }
  return (
    <ul>
      {earnings.map((line, index) => (
        // The wire order is the stored order and there is no id to key on,
        // and this list is never reordered or edited — it is read-only.
        <li key={index}>Taxable allowance {centsText(line.amountCents)}</li>
      ))}
    </ul>
  );
}

function Member({
  member,
  employerId,
  payrollRunId,
  runIsFinalized,
}: {
  member: PayrollRunMemberDto;
  employerId: string;
  payrollRunId: string;
  runIsFinalized: boolean;
}) {
  return (
    <li>
      <h3>{member.fullName}</h3>

      {runIsFinalized ? (
        <FinalizedEarnings earnings={member.earnings} />
      ) : (
        <EarningsForm
          payrollRunId={payrollRunId}
          employmentId={member.employmentId}
          earnings={member.earnings}
        />
      )}

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

/** `announce` only when this list is what the Operator's own Finalize just
 * answered. On a plain load of an already-finalized run it is standing
 * content that was on the screen before they read anything, and marking it a
 * status would make a screen reader read it out on every visit — the same
 * distinction `Member` keeps between a blocker and a refusal. */
function FinalizedLinks({
  run,
  employerId,
  links,
  announce,
}: {
  run: PayrollRunDetailResponse;
  employerId: string;
  links: { employmentId: string; finalizedPayrollId: string }[];
  announce: boolean;
}) {
  return (
    <div role={announce ? 'status' : undefined}>
      <p>Finalized. Open each person’s finalized payroll:</p>
      <ul>
        {links.map((entry) => {
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
  onCalculateAgain,
  calculateIsPending,
  finalize,
  confirming,
  setConfirming,
}: {
  run: PayrollRunDetailResponse;
  employerId: string;
  onCalculateAgain: () => void;
  calculateIsPending: boolean;
  finalize: ReturnType<typeof useFinalizePayrollRun>;
  confirming: boolean;
  setConfirming: (confirming: boolean) => void;
}) {
  const navigate = useNavigate();
  const [finalizedLinks, setFinalizedLinks] = useState<
    { employmentId: string; finalizedPayrollId: string }[] | null
  >(null);
  // Set only when `payroll_run_already_finalized` carried neither a single
  // `finalizedPayrollId` nor a list this screen could read. The run itself
  // still knows — its own re-read (invalidated by `useFinalizePayrollRun`)
  // carries each member's `finalizedPayrollId` — so this says the true thing
  // while that lands, and never leaves the Operator looking at a Finalize
  // button that silently did nothing.
  const [alreadyFinalized, setAlreadyFinalized] = useState(false);

  // A keyboard Operator opening or closing the confirmation would otherwise
  // lose focus to the document body, because the button they just pressed is
  // the one React unmounts (§0's story 51). Focus follows the decision
  // instead: onto "Confirm finalize" when it appears, back onto "Finalize"
  // when they cancel. Only ever after the Operator's own click — the
  // `confirming` state changes for no other reason.
  const confirmRef = useRef<HTMLButtonElement>(null);
  const finalizeRef = useRef<HTMLButtonElement>(null);
  const hasConfirmed = useRef(false);
  useEffect(() => {
    if (confirming) {
      hasConfirmed.current = true;
      confirmRef.current?.focus();
    } else if (hasConfirmed.current) {
      finalizeRef.current?.focus();
    }
  }, [confirming]);

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
        } else if (finalizedPayrolls.length > 0) {
          setFinalizedLinks(finalizedPayrolls);
        } else {
          setAlreadyFinalized(true);
        }
      }
    }
  }

  const finalizedLinksFromRun = run.members.flatMap((member) =>
    member.finalizedPayrollId === null
      ? []
      : [{ employmentId: member.employmentId, finalizedPayrollId: member.finalizedPayrollId }],
  );
  const links = finalizedLinks ?? finalizedLinksFromRun;
  if (links.length > 0) {
    return (
      <FinalizedLinks
        run={run}
        employerId={employerId}
        links={links}
        announce={finalizedLinks !== null}
      />
    );
  }

  // Still never a red banner (§0.28): the run finalized, and the only thing
  // missing is where to go and look at it.
  if (alreadyFinalized) {
    return <p role="status">This payroll run is already finalized.</p>;
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
          <button
            type="button"
            ref={confirmRef}
            onClick={() => void handleFinalize()}
            disabled={finalize.isPending || calculateIsPending}
          >
            {finalize.isPending ? 'Finalizing…' : 'Confirm finalize'}
          </button>{' '}
          <button
            type="button"
            onClick={() => setConfirming(false)}
            disabled={finalize.isPending || calculateIsPending}
          >
            Cancel
          </button>
        </p>
      ) : (
        <p>
          <button
            type="button"
            ref={finalizeRef}
            onClick={() => setConfirming(true)}
            disabled={calculateIsPending || finalize.isPending}
          >
            Finalize
          </button>
        </p>
      )}

      {failureMessage !== null && (
        <p role="alert">
          {failureMessage}{' '}
          {offersCalculateAgain(finalize.error) && (
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
  const finalize = useFinalizePayrollRun(runId);
  const [confirmingFinalize, setConfirmingFinalize] = useState(false);

  if (run.isError && runWasNotFound(run.error)) {
    return <NotFound />;
  }

  async function handleCalculate() {
    if (finalize.isPending) {
      return;
    }
    // A calculation replaces the figures the Operator had confirmed. Close
    // that confirmation and retire a prior mismatch before the new result can
    // be finalized, so approval always follows a visible current calculation.
    setConfirmingFinalize(false);
    finalize.reset();
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
                disabled={calculate.isPending || finalize.isPending}
              >
                {calculate.isPending ? 'Calculating…' : 'Calculate'}
              </button>
            </p>
          )}

          {calculate.isError && <p role="alert">{calculateFailureMessage(calculate.error)}</p>}

          <Finalize
            run={run.data}
            employerId={employerId}
            onCalculateAgain={() => void handleCalculate()}
            calculateIsPending={calculate.isPending}
            finalize={finalize}
            confirming={confirmingFinalize}
            setConfirming={setConfirmingFinalize}
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
                  runIsFinalized={run.data.status === 'finalized'}
                />
              ))}
            </ul>
          )}
        </>
      )}
    </main>
  );
}
