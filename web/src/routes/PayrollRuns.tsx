// `/app/employers/:employerId/payroll` (issue #64, parent #59 Spec 3 of 3,
// §0.29). Every payroll run this Employer has, with its period, pay date and
// status, and a form that creates the next Ordinary one.
//
// The list is what `GET /api/employers/{e}/payroll-runs` returns, re-read
// after a successful create (`useCreatePayrollRun` invalidates it) rather
// than grown locally — the same reason `People.tsx` does (issue #62).

import { type SubmitEvent, useId, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { ApiError } from '../api/client';
import { requestIdOf } from '../api/refusal';
import type { CreatePayrollRunRequest, PayrollRunStatus } from '../api/types';
import { useCreatePayrollRun, usePayrollRuns } from '../payrollRuns/usePayrollRuns';

/** `pay_period_not_generated_by_the_pay_schedule` carries the period this
 * Employer's schedule does generate around the one that was asked for. A
 * refusal that names it is one an Operator can act on; the same reason
 * `unsupported_deductions_present` names its kinds. Read from `details`,
 * never parsed out of `message` (§0.23). */
function schedulesPeriodOf(details: unknown): { start: string; end: string } | null {
  if (typeof details !== 'object' || details === null) {
    return null;
  }
  const { schedulesPeriodStart: start, schedulesPeriodEnd: end } = details as {
    schedulesPeriodStart?: unknown;
    schedulesPeriodEnd?: unknown;
  };
  return typeof start === 'string' && typeof end === 'string' ? { start, end } : null;
}

function messageForRefusal(caught: unknown): string {
  if (!(caught instanceof ApiError)) {
    return 'Something went wrong. Please try again.';
  }

  switch (caught.code) {
    case 'pay_period_not_generated_by_the_pay_schedule': {
      const schedulesPeriod = schedulesPeriodOf(caught.details);
      return schedulesPeriod === null
        ? 'That period does not match this employer’s pay schedule.'
        : `That period does not match this employer’s pay schedule. The schedule’s period around that end date runs ${schedulesPeriod.start} to ${schedulesPeriod.end}.`;
    }

    // The Employer's pay schedule was moved after a period of this tax year
    // was already finalized, so no further period of that year can be run
    // (§4.2). Nothing on any screen this spec builds can undo that, so the
    // sentence says who has to.
    case 'pay_schedule_moved_within_tax_year':
      return 'This employer’s pay schedule changed part-way through the tax year, so no further run can be created for it. Ask whoever administers this employer to put the schedule back.';

    // The Employer went out of this Operator's reach between loading the
    // screen and submitting it — the same story `People.tsx`'s own
    // `messageForRefusal` tells for the same two codes.
    case 'not_found':
    case 'employer_not_found':
      return 'This employer is no longer available to you. Reload the page.';

    case 'forbidden':
      return 'Your role does not allow creating a payroll run.';

    case 'internal_error': {
      const requestId = requestIdOf(caught.details);
      return requestId === null
        ? 'Something went wrong. Please try again.'
        : `Something went wrong. Please try again, and quote reference ${requestId}.`;
    }

    default:
      return 'Something went wrong. Please try again.';
  }
}

function payrollRunsLoadFailureMessage(caught: unknown): string {
  if (caught instanceof ApiError && caught.code === 'internal_error') {
    const requestId = requestIdOf(caught.details);
    if (requestId !== null) {
      return `We could not load the payroll runs. Try again, and quote reference ${requestId} if the problem continues.`;
    }
  }
  return 'We could not load the payroll runs.';
}

function statusLabel(status: PayrollRunStatus): string {
  switch (status) {
    case 'draft':
      return 'Draft';
    case 'calculated':
      return 'Calculated';
    case 'finalized':
      return 'Finalized';
  }
}

export function PayrollRuns() {
  const payrollRuns = usePayrollRuns();
  const createPayrollRun = useCreatePayrollRun();
  const navigate = useNavigate();
  const [periodStart, setPeriodStart] = useState('');
  const [periodEnd, setPeriodEnd] = useState('');
  const [payDate, setPayDate] = useState('');
  const [error, setError] = useState<string | null>(null);
  const periodStartId = useId();
  const periodEndId = useId();
  const payDateId = useId();

  function edited() {
    setError(null);
  }

  async function handleSubmit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    if (createPayrollRun.isPending) {
      return;
    }

    setError(null);

    const request: CreatePayrollRunRequest = {
      period: { start: periodStart, end: periodEnd },
      payDate,
    };

    try {
      const created = await createPayrollRun.mutateAsync(request);
      // Straight to the run just created: "sees everyone it proposes to
      // pay" is this issue's own next sentence after "creates the next
      // ordinary one", not a separate trip back through the list.
      navigate(created.payrollRunId);
    } catch (caught) {
      setError(messageForRefusal(caught));
    }
  }

  return (
    <main>
      <h2>Payroll</h2>

      {payrollRuns.isPending && <p>Loading…</p>}

      {payrollRuns.isError && (
        <p role="alert">
          {payrollRunsLoadFailureMessage(payrollRuns.error)}{' '}
          <button type="button" onClick={() => void payrollRuns.refetch()}>
            Try again
          </button>
        </p>
      )}

      {payrollRuns.isSuccess &&
        (payrollRuns.data.payrollRuns.length === 0 ? (
          <p>No payroll runs yet.</p>
        ) : (
          <ul>
            {payrollRuns.data.payrollRuns.map((run) => (
              <li key={run.payrollRunId}>
                <Link to={run.payrollRunId}>
                  {run.period.start} to {run.period.end}, paid {run.payDate}
                </Link>{' '}
                — {statusLabel(run.status)}
              </li>
            ))}
          </ul>
        ))}

      <h3>Create an ordinary run</h3>
      <form onSubmit={handleSubmit}>
        <div>
          <label htmlFor={periodStartId}>Period start</label>
          <input
            id={periodStartId}
            type="date"
            required
            value={periodStart}
            onChange={(event) => {
              setPeriodStart(event.target.value);
              edited();
            }}
          />
        </div>
        <div>
          <label htmlFor={periodEndId}>Period end</label>
          <input
            id={periodEndId}
            type="date"
            required
            value={periodEnd}
            onChange={(event) => {
              setPeriodEnd(event.target.value);
              edited();
            }}
          />
        </div>
        <div>
          <label htmlFor={payDateId}>Pay date</label>
          <input
            id={payDateId}
            type="date"
            required
            value={payDate}
            onChange={(event) => {
              setPayDate(event.target.value);
              edited();
            }}
          />
        </div>
        {error !== null && <p role="alert">{error}</p>}
        <button type="submit" disabled={createPayrollRun.isPending}>
          {createPayrollRun.isPending ? 'Creating…' : 'Create run'}
        </button>
      </form>
    </main>
  );
}
