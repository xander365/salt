// `/app/employers/:employerId/payroll/:runId` (issue #64, parent #59 Spec 3
// of 3, §0.29/§0.31; rebuilt for issue #88). Every member one payroll run
// proposes to pay, by name, with their current Earning lines and why they
// cannot be paid yet, if at all.
//
// `blockers` is read straight off this screen's own `GET` (`usePayrollRun`)
// on every render, including a reload — never cached separately, so a fact
// recorded on the Employment screen and this screen returned to always
// agree (§0.31, issue #64's own Deep Instructions). This screen computes no
// readiness of its own: an empty `blockers` list is the server's own answer
// that a member is ready, and nothing else here decides that.

import { type ReactNode, useEffect, useRef, useState } from 'react';
import { ArrowLeft, CircleAlert, CircleCheck, RefreshCw } from 'lucide-react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import { ApiError } from '../api/client';
import { requestIdOf } from '../api/refusal';
import type {
  DeductionLineDto,
  EarningLineDto,
  MemberProposalRefreshDto,
  PayrollRunBlockerDto,
  PayrollRunDetailResponse,
  PayrollRunMemberDto,
  StandingItemProposalDto,
  StandingItemsChangedDto,
} from '../api/types';
import { useEmployerId } from '../employments/useEmployments';
import { humanDate, humanDateRange } from '../format';
import { moneyDisplayText } from '../money';
import { Money } from '../components/Money';
import {
  useCalculatePayrollRun,
  useFinalizePayrollRun,
  usePayrollRun,
  useRefreshStandingProposals,
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
import { StandingLines } from './payroll/StandingLines';
import { Button } from '../components/ui/button';
import { LoadingState } from '../components/states/LoadingState';
import { EmptyState } from '../components/states/EmptyState';
import { FailedRequestState } from '../components/states/FailedRequestState';
import { BlockedItem } from '../components/states/BlockedItem';
import { StaleBanner } from '../components/states/StaleBanner';

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

function PayrollAlert({ children }: { children: ReactNode }) {
  return (
    <p role="alert" className="flex items-start gap-1.5 text-sm text-destructive">
      <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
      <span>{children}</span>
    </p>
  );
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

/** A finalized run's Earnings and voluntary deductions, read back rather
 * than edited. `PUT .../pay-lines` refuses once history is written
 * (`payroll_run_already_finalized`), so offering the editor here would be a
 * form whose every save is a refusal — the same reason Finalize and
 * Calculate are both gone by this point. */
function FinalizedEarnings({
  earnings,
  deductions,
}: {
  earnings: EarningLineDto[];
  deductions: DeductionLineDto[];
}) {
  if (earnings.length === 0 && deductions.length === 0) {
    return (
      <p className="text-sm text-muted-foreground">
        No earnings beyond basic pay, and no deductions.
      </p>
    );
  }
  return (
    <ul className="text-sm">
      {earnings.map((line, index) => (
        // The wire order is the stored order and there is no id to key on,
        // and this list is never reordered or edited — it is read-only.
        <li key={`earning-${index}`}>
          {line.kind === 'taxableAllowance' ? (
            <>
              Taxable allowance — {line.label ?? 'unlabelled'} <Money cents={line.amountCents} />
            </>
          ) : (
            // Hours and a multiplier, never money: the instruction is what
            // was stored, and the money it produced is in the figures and
            // in the workings on the finalized payslip.
            <>
              Overtime — {line.label ?? 'unlabelled'}: {line.hours} hours &times; {line.multiplier}
            </>
          )}
        </li>
      ))}
      {deductions.map((line, index) => (
        <li key={`deduction-${index}`}>
          Medical aid premium <Money cents={line.amountCents} />
        </li>
      ))}
    </ul>
  );
}

function standingItemsChangedIsEmpty(changed: StandingItemsChangedDto): boolean {
  return changed.added.length === 0 && changed.changed.length === 0 && changed.ended.length === 0;
}

function proposalSentence(item: StandingItemProposalDto): string {
  const line =
    item.kind === 'taxableAllowance' ? `Taxable allowance — ${item.label}` : 'Medical aid premium';
  return `${line} ${moneyDisplayText(item.amountCents)}, standing since ${humanDate(item.effectiveFrom)}`;
}

/** Names a deliberately preserved line in a refresh report. The refresh
 * response intentionally carries only StandingPayItem ids for these two
 * lists, so resolve those ids against the run detail already on screen
 * instead of reducing the result to an ambiguous count. */
function standingLineSentence(member: PayrollRunMemberDto, standingPayItemId: string): string {
  const line = [...member.earnings, ...member.deductions, ...member.removedPayLines].find(
    (candidate) =>
      'standingPayItemId' in candidate && candidate.standingPayItemId === standingPayItemId,
  );
  if (line === undefined) {
    return `standing item ${standingPayItemId}`;
  }
  if (line.kind === 'overtime') {
    // A StandingPayItem can never be overtime (D14). Keep an honest fallback
    // for a future wire shape rather than silently dropping the report row.
    return `overtime — ${line.label ?? 'unlabelled'}`;
  }
  const name =
    line.kind === 'taxableAllowance'
      ? `taxable allowance — ${line.label ?? 'unlabelled'}`
      : 'medical aid premium';
  return `${name} ${moneyDisplayText(line.amountCents)}`;
}

function refreshFailureMessage(caught: unknown): string {
  if (!(caught instanceof ApiError)) {
    return 'We could not refresh these proposals. Please try again.';
  }
  switch (caught.code) {
    case 'payroll_run_already_finalized':
      return 'This payroll run was finalized before the proposals could be refreshed. Reload the page.';
    case 'payroll_run_is_not_ordinary':
      return 'Correction runs do not have standing proposals to refresh.';
    case 'internal_error': {
      const requestId = requestIdOf(caught.details);
      return requestId === null
        ? 'We could not refresh these proposals. Please try again.'
        : `We could not refresh these proposals. Try again, and quote reference ${requestId} if the problem continues.`;
    }
    default:
      return 'We could not refresh these proposals. Please try again.';
  }
}

/**
 * The run-level change banner and refresh action (issue #80, §0, §D-6). A
 * draft never refreshes silently: this reports what the standing records
 * now say that the draft does not yet, per person, and refreshing is one
 * explicit click away, never automatic.
 *
 * The report after a refresh (`role="status"`) says what was added and
 * updated, and what was deliberately left alone — an empty `members` array
 * on the response means nothing changed and nothing was written, which is
 * said plainly rather than left silent.
 */
function StandingItemsChangedBanner({
  run,
  payrollRunId,
}: {
  run: PayrollRunDetailResponse;
  payrollRunId: string;
}) {
  const refresh = useRefreshStandingProposals(payrollRunId);
  const [report, setReport] = useState<MemberProposalRefreshDto[] | null>(null);

  const membersWithChanges = run.members.filter(
    (member) => !standingItemsChangedIsEmpty(member.standingItemsChanged),
  );

  async function handleRefresh() {
    try {
      const result = await refresh.mutateAsync();
      setReport(result.members);
    } catch {
      // `refresh.isError`/`refresh.error` already carry this for the
      // render below — nothing further to do here.
    }
  }

  return (
    <>
      {membersWithChanges.length > 0 && (
        <StaleBanner>
          <div className="flex flex-col gap-2">
            <p className="font-medium text-foreground">
              The standing records changed since these proposals were made.
            </p>
            <ul className="flex flex-col gap-1">
              {membersWithChanges.map((member) => (
                <li key={member.employmentId}>
                  <span className="text-foreground">{member.fullName}</span>
                  {member.standingItemsChanged.added.length > 0 && (
                    <>
                      {' '}
                      — Added: {member.standingItemsChanged.added.map(proposalSentence).join(', ')}.
                    </>
                  )}
                  {member.standingItemsChanged.changed.length > 0 && (
                    <>
                      {' '}
                      Changed:{' '}
                      {member.standingItemsChanged.changed.map(proposalSentence).join(', ')}.
                    </>
                  )}
                  {member.standingItemsChanged.ended.length > 0 && (
                    <>
                      {' '}
                      Ended but still on this run:{' '}
                      {member.standingItemsChanged.ended.map(proposalSentence).join(', ')} — remove
                      it for this run if it should not be paid.
                    </>
                  )}
                </li>
              ))}
            </ul>
            <Button
              type="button"
              size="sm"
              onClick={() => void handleRefresh()}
              disabled={refresh.isPending}
              className="self-start"
            >
              <RefreshCw className="size-4" aria-hidden="true" />
              {refresh.isPending ? 'Refreshing…' : 'Refresh proposals'}
            </Button>
            {refresh.isError && <PayrollAlert>{refreshFailureMessage(refresh.error)}</PayrollAlert>}
          </div>
        </StaleBanner>
      )}

      {report !== null && (
        <div role="status" className="flex flex-col gap-2 rounded-md border p-3 text-sm">
          {report.length === 0 ? (
            <p>Nothing changed. Nothing was written.</p>
          ) : (
            <ul className="flex flex-col gap-1">
              {report.map((member) => {
                const runMember = run.members.find(
                  (candidate) => candidate.employmentId === member.employmentId,
                );
                const memberName = runMember?.fullName ?? member.employmentId;
                return (
                  <li key={member.employmentId}>
                    <span className="font-medium">{memberName}</span>
                    {member.added.length > 0 && (
                      <> — Added: {member.added.map(proposalSentence).join(', ')}.</>
                    )}
                    {member.updated.length > 0 && (
                      <> Updated: {member.updated.map(proposalSentence).join(', ')}.</>
                    )}
                    {member.keptOverridden.length > 0 && (
                      <>
                        {' '}
                        Left alone, changed for this run:{' '}
                        {member.keptOverridden
                          .map((id) =>
                            runMember === undefined ? id : standingLineSentence(runMember, id),
                          )
                          .join(', ')}
                        .
                      </>
                    )}
                    {member.keptRemoved.length > 0 && (
                      <>
                        {' '}
                        Left alone, removed for this run:{' '}
                        {member.keptRemoved
                          .map((id) =>
                            runMember === undefined ? id : standingLineSentence(runMember, id),
                          )
                          .join(', ')}
                        .
                      </>
                    )}
                    {member.endedStillProposed.length > 0 && (
                      <>
                        {' '}
                        Still on this run though ended:{' '}
                        {member.endedStillProposed.map(proposalSentence).join(', ')}.
                      </>
                    )}
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      )}
    </>
  );
}

function Member({
  member,
  employerId,
  payrollRunId,
  runIsFinalized,
  figuresAreStale,
  onEarningsChanged,
}: {
  member: PayrollRunMemberDto;
  employerId: string;
  payrollRunId: string;
  runIsFinalized: boolean;
  figuresAreStale: boolean;
  onEarningsChanged: () => void;
}) {
  return (
    <li className="flex flex-col gap-4 rounded-xl border bg-card p-6 text-card-foreground shadow-sm">
      <h3 className="text-lg font-semibold">{member.fullName}</h3>

      {runIsFinalized ? (
        <FinalizedEarnings earnings={member.earnings} deductions={member.deductions} />
      ) : (
        <EarningsForm
          payrollRunId={payrollRunId}
          employmentId={member.employmentId}
          earnings={member.earnings}
          deductions={member.deductions}
          onChanged={onEarningsChanged}
        />
      )}

      <StandingLines
        employerId={employerId}
        payrollRunId={payrollRunId}
        employmentId={member.employmentId}
        earnings={member.earnings}
        deductions={member.deductions}
        removedPayLines={member.removedPayLines}
        runIsFinalized={runIsFinalized}
        onChanged={onEarningsChanged}
      />

      {/* Only BasicPay is prorated (issue #79's own Deep Instructions): a
          joiner's or leaver's standing items are proposed at their full
          amount, and Salt has no rule for prorating them, so the worksheet
          says so rather than let a full allowance beside a smaller salary
          look like a mistake Salt made. */}
      {member.basicPayProrated && (
        <p className="text-sm text-muted-foreground">
          <span className="font-medium text-foreground">Part period.</span> Basic pay is prorated by
          the days employed in this period. Nothing else is: standing items and other lines are paid
          at their full amount.{' '}
          {!runIsFinalized && <>Change a line on this run if it should be less.</>}
        </p>
      )}

      {/* The hours column slot (issue #88's own Deep Instructions): hourly
          pay is out of this milestone, but the worksheet already reserves
          the place for it, so the later change is a fill rather than a
          redesign. */}
      <div className="text-sm text-muted-foreground">
        <span className="font-medium text-foreground">Hours:</span> —
      </div>

      {/* The two "present" blockers matter as much as the two "unknown"
          ones. An empty list says only that no standing fact is currently
          blocking calculation — a fresh calculation may still refuse for a
          calculation-time reason. */}
      {member.blockers.length === 0 ? (
        <p className="text-sm text-muted-foreground">No standing blockers.</p>
      ) : (
        <ul className="flex flex-col gap-2">
          {/* Not `role="alert"`: a blocker is standing content that is
              already on the screen when it loads, not something that just
              happened. Marking each one assertive would make a screen
              reader interrupt itself once per blocker on every load. */}
          {member.blockers.map((blocker) => (
            <BlockedItem key={blocker.code} to={blockerFixPath(employerId, member, blocker)}>
              {blockerSentence(blocker)}
            </BlockedItem>
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
          read as a member with nothing wrong at all (§0.31).

          Stale figures are *replaced* by the warning, never shown beneath
          it (§0's story 75: "say so and hide the old figures"). Earnings
          saved since the last Calculate mean every figure here predates the
          edit, and a number an Operator can still read is a number they can
          still act on — so the only honest thing on screen is the sentence
          saying they are gone until Calculate runs again. */}
      {/* `pay_lines_saved` is the server's own record that a save retired
          this member's figures (issue #77), so the sentence survives a
          reload; `figuresAreStale` covers the moment before the refetch. */}
      {member.calculationState === 'pay_lines_saved' ? (
        <StaleBanner>
          Earnings changed since these figures were calculated. The figures are hidden until you
          calculate again.
        </StaleBanner>
      ) : member.figures === null ? (
        <p className="text-sm text-muted-foreground">
          No figures yet. Calculate this run to see them.
        </p>
      ) : figuresAreStale ? (
        <StaleBanner>
          Earnings changed since these figures were calculated. The figures are hidden until you
          calculate again.
        </StaleBanner>
      ) : (
        <Figures figures={member.figures} />
      )}

      {/* `role="alert"`, unlike a blocker: this is what the Calculate an
          Operator just ran said about this member, not standing content
          already on the screen when it loaded. */}
      {member.refusal !== null && (
        <PayrollAlert>Could not calculate: {refusalSentence(member.refusal)}</PayrollAlert>
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
    <div role={announce ? 'status' : undefined} className="flex flex-col gap-2">
      <p className="flex items-center gap-2 text-sm font-medium text-success">
        <CircleCheck className="size-4" aria-hidden="true" />
        Finalized. Open each person’s finalized payroll:
      </p>
      <ul className="flex flex-col gap-1">
        {links.map((entry) => {
          const member = run.members.find(
            (candidate) => candidate.employmentId === entry.employmentId,
          );
          return (
            <li key={entry.employmentId}>
              <Link
                to={finalizedPayrollPath(employerId, entry.finalizedPayrollId)}
                className="text-sm font-medium text-primary hover:underline"
              >
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
    <div className="flex flex-col gap-2">
      {confirming ? (
        <p className="flex flex-wrap items-center gap-2 text-sm">
          This creates immutable payroll history for {run.members.length}{' '}
          {run.members.length === 1 ? 'person' : 'people'}.
          <Button
            type="button"
            ref={confirmRef}
            onClick={() => void handleFinalize()}
            disabled={finalize.isPending || calculateIsPending}
          >
            {finalize.isPending ? 'Finalizing…' : 'Confirm finalize'}
          </Button>
          <Button
            type="button"
            variant="outline"
            onClick={() => setConfirming(false)}
            disabled={finalize.isPending || calculateIsPending}
          >
            Cancel
          </Button>
        </p>
      ) : (
        <p>
          <Button
            type="button"
            ref={finalizeRef}
            onClick={() => setConfirming(true)}
            disabled={calculateIsPending || finalize.isPending}
          >
            Finalize
          </Button>
        </p>
      )}

      {failureMessage !== null && offersCalculateAgain(finalize.error) && (
        <FailedRequestState
          message={failureMessage}
          onRetry={onCalculateAgain}
          retrying={calculateIsPending}
          retryLabel="Calculate again"
        />
      )}
      {/* No retry action for a finalize failure that is not one of the
          "calculate again" codes (§0.26/§0.28) — offering one here would be
          a button that repeats the wrong request, since Finalize itself
          (above) is what an Operator retries for those. */}
      {failureMessage !== null && !offersCalculateAgain(finalize.error) && (
        <PayrollAlert>{failureMessage}</PayrollAlert>
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
  // Key staleness by run as well as Employment so navigating directly from
  // one run to another cannot carry a warning onto the new worksheet.
  const [staleCalculationKeys, setStaleCalculationKeys] = useState<Set<string>>(() => new Set());

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
      setStaleCalculationKeys(new Set());
    } catch {
      // `calculate.isError` and `calculate.error` already carry this for
      // the render below — nothing further to do here.
    }
  }

  return (
    <main className="flex flex-col gap-6">
      {/* Named distinctly from the persistent "Payroll" nav link beside it
          (both are on screen at once) — the same reason `Employment.tsx`'s
          own back link is. */}
      <Link
        to=".."
        relative="path"
        aria-label="Back to Payroll"
        className="flex w-fit items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
      >
        <ArrowLeft className="size-4" aria-hidden="true" />
        Payroll
      </Link>

      {run.isPending && <LoadingState label="Loading payroll run…" />}

      {/* An Operator arriving back from clearing a blocker sees this run's
          last response first, while the re-read that decides whether the
          blocker is really gone is still in flight. Saying so is the honest
          version of that moment; showing an old blocker in silence is not. */}
      {run.isSuccess && run.isFetching && <StaleBanner>Refreshing…</StaleBanner>}

      {run.isError && (
        <FailedRequestState
          message={loadFailureMessage(run.error)}
          onRetry={() => void run.refetch()}
          retrying={run.isFetching}
        />
      )}

      {run.isSuccess && (
        <>
          <div>
            <h2 className="text-2xl font-semibold tracking-tight">
              {humanDateRange(run.data.period.start, run.data.period.end)}
            </h2>
            <p className="text-sm text-muted-foreground">Pay date: {humanDate(run.data.payDate)}</p>
          </div>

          {/* Finalize belongs to issue #66, and is offered there only when
              `status` itself says so (§0's Deep Instructions: the browser
              never decides that). Calculate has no such gate of its own —
              it is always safe to run again, on a Draft or a Calculated run
              alike — except once history is written, and a Finalized run's
              own Calculate call would refuse that itself if ever clicked
              from a stale screen. */}
          {run.data.status !== 'finalized' && (
            <div>
              <Button
                type="button"
                onClick={() => void handleCalculate()}
                disabled={calculate.isPending || finalize.isPending}
              >
                {calculate.isPending ? 'Calculating…' : 'Calculate'}
              </Button>
            </div>
          )}

          {calculate.isError && (
            <PayrollAlert>{calculateFailureMessage(calculate.error)}</PayrollAlert>
          )}

          <Finalize
            run={run.data}
            employerId={employerId}
            onCalculateAgain={() => void handleCalculate()}
            calculateIsPending={calculate.isPending}
            finalize={finalize}
            confirming={confirmingFinalize}
            setConfirming={setConfirmingFinalize}
          />

          <StandingItemsChangedBanner run={run.data} payrollRunId={runId} />

          {run.data.members.length === 0 ? (
            <EmptyState>No one is proposed to be paid on this run.</EmptyState>
          ) : (
            <ul className="flex flex-col gap-4">
              {run.data.members.map((member) => (
                <Member
                  key={member.employmentId}
                  member={member}
                  employerId={employerId}
                  payrollRunId={runId}
                  runIsFinalized={run.data.status === 'finalized'}
                  figuresAreStale={staleCalculationKeys.has(`${runId}:${member.employmentId}`)}
                  onEarningsChanged={() =>
                    setStaleCalculationKeys((current) => {
                      const next = new Set(current);
                      next.add(`${runId}:${member.employmentId}`);
                      return next;
                    })
                  }
                />
              ))}
            </ul>
          )}
        </>
      )}
    </main>
  );
}
