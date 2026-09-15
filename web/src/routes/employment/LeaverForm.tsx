// `PUT .../end-date` (issue #81, parent #70). An operator records that
// someone has left, with a stated reason. Their last eligible month is paid
// as an ordinary payroll under the rules that already exist — this form
// records only the date and the reason; the proration, the standing-item
// full-amount rule and run membership are all handled elsewhere, unchanged.
//
// Leave payout, notice pay and severance are named here, on screen, as
// refused rather than left as an absent feature (D23): an operator who
// looks for them must be told why they are not here, never nudged into
// typing one into an allowance instead.

import { type SubmitEvent, useId, useState } from 'react';
import { ApiError } from '../../api/client';
import { sharedFactRefusalMessage } from '../../api/refusal';
import { humanDate } from '../../format';
import { useRecordEmploymentEndDate } from '../../employments/useEmploymentFacts';
import { type FieldError, fieldErrorProps } from './fieldError';
import { Button } from '../../components/ui/button';
import { Input } from '../../components/ui/input';
import { Label } from '../../components/ui/label';
import { ValidationError } from '../../components/states/ValidationError';

function messageForRefusal(caught: unknown): string {
  const shared = sharedFactRefusalMessage(caught);
  if (shared !== null) {
    return shared;
  }

  const error = caught as ApiError;
  switch (error.code) {
    case 'employment_end_date_reason_cannot_be_empty':
      return 'Enter a reason for this change.';

    case 'employment_ends_before_it_starts': {
      const startDate = (error.details as { startDate?: unknown } | null)?.startDate;
      return typeof startDate === 'string'
        ? `This employment started ${startDate}. Choose an end date on or after it.`
        : 'The end date must be on or after this employment’s start date.';
    }

    // Issue #81's own headline refusal: recording a leaver must never
    // invalidate paid history, so this is refused outright, naming exactly
    // which period is already paid.
    case 'employment_end_date_precedes_paid_periods': {
      const periods = (error.details as { periods?: { start?: unknown; end?: unknown }[] } | null)
        ?.periods;
      const named = Array.isArray(periods)
        ? periods
            .filter(
              (period): period is { start: string; end: string } =>
                typeof period.start === 'string' && typeof period.end === 'string',
            )
            .map((period) => `${period.start} to ${period.end}`)
            .join(', ')
        : null;
      return named !== null && named.length > 0
        ? `This date falls before pay already finalized for ${named}. Choose a later date, or contact support if that pay was wrong.`
        : 'This date falls before pay already finalized for this employee. Choose a later date.';
    }

    default:
      return 'Something went wrong. Please try again.';
  }
}

type Field = 'endDate' | 'reason';

export function LeaverForm({ employmentId }: { employmentId: string }) {
  const recordEndDate = useRecordEmploymentEndDate(employmentId);
  const [endDate, setEndDate] = useState('');
  const [reason, setReason] = useState('');
  const [fieldError, setFieldError] = useState<FieldError<Field> | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const endDateId = useId();
  const reasonId = useId();
  const errorId = useId();
  const headingId = useId();

  function edited() {
    setFieldError(null);
    setError(null);
    setSaved(null);
  }

  async function handleSubmit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    if (recordEndDate.isPending) {
      return;
    }

    setError(null);
    setSaved(null);

    if (endDate === '') {
      setFieldError({ field: 'endDate', message: 'Enter the last day this employee is paid.' });
      return;
    }
    if (reason.trim() === '') {
      setFieldError({ field: 'reason', message: 'Enter a reason for this change.' });
      return;
    }
    setFieldError(null);

    try {
      await recordEndDate.mutateAsync({ endDate, reason: reason.trim() });
      setSaved(`Recorded: employment ends ${humanDate(endDate)}.`);
    } catch (caught) {
      setError(messageForRefusal(caught));
    }
  }

  return (
    <section
      id="leaver"
      aria-labelledby={headingId}
      className="rounded-xl border bg-card p-6 text-card-foreground shadow-sm"
    >
      <h3 id={headingId} className="mb-1 text-base font-semibold">
        Leaver
      </h3>
      <p className="mb-4 text-sm text-muted-foreground">
        Their last eligible pay period is paid as an ordinary month, prorated by employed calendar
        days if it is a part month. Standing pay items are proposed at their full amount, not
        prorated. Once recorded, this employee is never proposed on a later payroll run.
      </p>
      <form onSubmit={handleSubmit} className="flex flex-col gap-4 sm:max-w-sm">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={endDateId}>Last day employed</Label>
          <Input
            id={endDateId}
            type="date"
            required
            {...fieldErrorProps(fieldError, 'endDate', errorId)}
            value={endDate}
            onChange={(event) => {
              setEndDate(event.target.value);
              edited();
            }}
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={reasonId}>Reason</Label>
          <Input
            id={reasonId}
            type="text"
            required
            placeholder="e.g. resigned, dismissed, contract ended"
            {...fieldErrorProps(fieldError, 'reason', errorId)}
            value={reason}
            onChange={(event) => {
              setReason(event.target.value);
              edited();
            }}
          />
        </div>

        {(fieldError !== null || error !== null) && (
          <ValidationError id={errorId}>{fieldError?.message ?? error}</ValidationError>
        )}
        <Button type="submit" disabled={recordEndDate.isPending} className="self-start">
          {recordEndDate.isPending ? 'Saving…' : 'Record end date'}
        </Button>
      </form>

      <p role="status" className="mt-3 text-sm text-success">
        {saved ?? ''}
      </p>

      {/* D23: named as refused, on screen, so an operator never has to
          wonder whether these are simply missing from this build. */}
      <p className="mt-4 border-t pt-4 text-sm text-muted-foreground">
        Leave payout, notice pay and severance are not calculated by Salt this milestone. Salt
        refuses any pay line labelled with one of these names — record and pay them outside Salt.
      </p>
    </section>
  );
}
