// `/app/employers/:employerId/payroll` (issue #64, parent #59 Spec 3 of 3,
// §0.29; rebuilt for issue #88). Every payroll run this Employer has, with
// its period, pay date and status, and a form that creates the next
// Ordinary one.
//
// The list is what `GET /api/employers/{e}/payroll-runs` returns, re-read
// after a successful create (`useCreatePayrollRun` invalidates it) rather
// than grown locally — the same reason `People.tsx` does (issue #62).

import { type SubmitEvent, useId, useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { CalendarPlus } from 'lucide-react';
import { ApiError } from '../api/client';
import { requestIdOf } from '../api/refusal';
import type { CreatePayrollRunRequest, PayrollRunStatus } from '../api/types';
import { useCreatePayrollRun, usePayrollRuns } from '../payrollRuns/usePayrollRuns';
import { humanDate, humanDateRange } from '../format';
import { Button } from '../components/ui/button';
import { Input } from '../components/ui/input';
import { Label } from '../components/ui/label';
import { Badge } from '../components/ui/badge';
import { Card, CardContent, CardHeader, CardTitle } from '../components/ui/card';
import { LoadingState } from '../components/states/LoadingState';
import { EmptyState } from '../components/states/EmptyState';
import { FailedRequestState } from '../components/states/FailedRequestState';
import { ValidationError } from '../components/states/ValidationError';

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

function statusVariant(status: PayrollRunStatus): 'outline' | 'secondary' | 'default' {
  switch (status) {
    case 'draft':
      return 'outline';
    case 'calculated':
      return 'secondary';
    case 'finalized':
      return 'default';
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
    <main className="flex flex-col gap-6">
      <h2 className="text-2xl font-semibold tracking-tight">Payroll</h2>

      {payrollRuns.isPending && <LoadingState label="Loading payroll runs…" />}

      {payrollRuns.isError && (
        <FailedRequestState
          message={payrollRunsLoadFailureMessage(payrollRuns.error)}
          onRetry={() => void payrollRuns.refetch()}
          retrying={payrollRuns.isFetching}
        />
      )}

      {payrollRuns.isSuccess &&
        (payrollRuns.data.payrollRuns.length === 0 ? (
          <EmptyState>No payroll runs yet. Create the first one below.</EmptyState>
        ) : (
          <ul className="flex flex-col divide-y rounded-lg border">
            {payrollRuns.data.payrollRuns.map((run) => (
              <li key={run.payrollRunId}>
                <Link
                  to={run.payrollRunId}
                  className="flex flex-wrap items-center justify-between gap-2 px-4 py-3 text-sm hover:bg-accent hover:text-accent-foreground"
                >
                  <span className="font-medium">
                    {humanDateRange(run.period.start, run.period.end)}, paid{' '}
                    {humanDate(run.payDate)}
                  </span>
                  <Badge variant={statusVariant(run.status)}>{statusLabel(run.status)}</Badge>
                </Link>
              </li>
            ))}
          </ul>
        ))}

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-base">
            <CalendarPlus className="size-4" aria-hidden="true" />
            Create an ordinary run
          </CardTitle>
        </CardHeader>
        <CardContent>
          <form onSubmit={handleSubmit} className="flex flex-col gap-4 sm:max-w-md">
            <div className="grid gap-4 sm:grid-cols-3">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor={periodStartId}>Period start</Label>
                <Input
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
              <div className="flex flex-col gap-1.5">
                <Label htmlFor={periodEndId}>Period end</Label>
                <Input
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
              <div className="flex flex-col gap-1.5">
                <Label htmlFor={payDateId}>Pay date</Label>
                <Input
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
            </div>
            {error !== null && <ValidationError>{error}</ValidationError>}
            <Button type="submit" disabled={createPayrollRun.isPending} className="sm:self-start">
              {createPayrollRun.isPending ? 'Creating…' : 'Create run'}
            </Button>
          </form>
        </CardContent>
      </Card>
    </main>
  );
}
