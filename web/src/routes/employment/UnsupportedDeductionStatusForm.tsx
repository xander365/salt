// `POST .../unsupported-deductions` (issue #63). Whether the employee has
// deductions Salt does not calculate — the same unasked-vs-recorded-"none"
// rule as `PriorEmploymentForm`: nothing is checked until the Operator
// chooses, and the line under the form says so in words.

import { type SubmitEvent, useId, useState } from 'react';
import { ApiError } from '../../api/client';
import { sharedFactRefusalMessage } from '../../api/refusal';
import type {
  DeclareUnsupportedDeductionStatusRequest,
  UnsupportedDeductionKindCode,
  UnsupportedDeductionStatusValue,
} from '../../api/types';
import { useDeclareUnsupportedDeductionStatus } from '../../employments/useEmploymentFacts';
import { type FieldError, fieldErrorProps } from './fieldError';
import { KINDS } from './unsupportedDeductionKinds';
import { Button } from '../../components/ui/button';
import { Input } from '../../components/ui/input';
import { Label } from '../../components/ui/label';
import { ValidationError } from '../../components/states/ValidationError';

/** The chosen kinds in `KINDS`' own order, so two Operators who tick the
 * same boxes read the same sentence back. */
function chosenKindLabels(kinds: Set<UnsupportedDeductionKindCode>): string[] {
  return KINDS.filter((kind) => kinds.has(kind.code)).map((kind) => kind.label.toLowerCase());
}

function messageForRefusal(caught: unknown): string {
  const shared = sharedFactRefusalMessage(caught);
  if (shared !== null) {
    return shared;
  }

  const error = caught as ApiError;
  switch (error.code) {
    case 'effective_from_not_a_period_start': {
      const nextValid = (error.details as { nextValidEffectiveFrom?: unknown } | null)
        ?.nextValidEffectiveFrom;
      return typeof nextValid === 'string'
        ? `This must start on the first day of a pay period. The next one starts ${nextValid}.`
        : 'This must start on the first day of one of this employer’s pay periods.';
    }

    case 'unsupported_deduction_declaration_reason_cannot_be_empty':
      return 'Enter a reason.';

    case 'master_data_divergence_not_acknowledged':
      return 'This would change pay for a period already paid, which this screen cannot confirm. Contact support.';

    default:
      return 'Something went wrong. Please try again.';
  }
}

/** The fields this form validates for shape, and so the fields one of its
 * own complaints can be about. */
type Field = 'status' | 'kinds' | 'reason';

/** Named once and used twice, for the same reason as
 * `PriorEmploymentForm`'s: `role="radiogroup"` displaces the `legend` as the
 * group's accessible name. */
const STATUS_QUESTION = 'Does this employee have any deductions Salt does not support?';

export function UnsupportedDeductionStatusForm({ employmentId }: { employmentId: string }) {
  const declareStatus = useDeclareUnsupportedDeductionStatus(employmentId);
  const [effectiveFrom, setEffectiveFrom] = useState('');
  const [status, setStatus] = useState<UnsupportedDeductionStatusValue | null>(null);
  const [kinds, setKinds] = useState<Set<UnsupportedDeductionKindCode>>(new Set());
  const [reason, setReason] = useState('');
  const [fieldError, setFieldError] = useState<FieldError<Field> | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const effectiveFromId = useId();
  const reasonId = useId();
  const errorId = useId();
  const headingId = useId();

  // Clears the confirmation too: it quotes the date, the status and the
  // kinds, so leaving it up beside changed fields would describe a
  // declaration nobody made.
  function edited() {
    setFieldError(null);
    setError(null);
    setSaved(null);
  }

  function toggleKind(code: UnsupportedDeductionKindCode) {
    setKinds((current) => {
      const next = new Set(current);
      if (next.has(code)) {
        next.delete(code);
      } else {
        next.add(code);
      }
      return next;
    });
    edited();
  }

  async function handleSubmit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    if (declareStatus.isPending) {
      return;
    }

    setError(null);
    setSaved(null);

    if (status === null) {
      setFieldError({
        field: 'status',
        message: 'Choose whether this employee has any unsupported deductions.',
      });
      return;
    }
    if (status === 'present' && kinds.size === 0) {
      setFieldError({ field: 'kinds', message: 'Choose at least one kind of deduction.' });
      return;
    }
    // Shape only. The server refuses a blank reason unconditionally, and
    // saying so here saves a round trip; it is never a substitute for that
    // refusal.
    if (reason.trim() === '') {
      setFieldError({ field: 'reason', message: 'Enter a reason.' });
      return;
    }
    setFieldError(null);

    const chosen = chosenKindLabels(kinds);
    const request: DeclareUnsupportedDeductionStatusRequest = {
      effectiveFrom,
      status,
      reason,
      ...(status === 'present' ? { kinds: [...kinds] } : {}),
    };

    try {
      await declareStatus.mutateAsync(request);
      setSaved(
        status === 'confirmed_none'
          ? `Recorded: no unsupported deductions from ${effectiveFrom}.`
          : `Recorded: unsupported deductions from ${effectiveFrom} — ${chosen.join(', ')}.`,
      );
    } catch (caught) {
      setError(messageForRefusal(caught));
    }
  }

  return (
    // Named so a payroll run's `unsupported_deduction_status_unknown` and
    // `unsupported_deductions_present` blockers can link straight here
    // (issue #64's own Deep Instructions).
    <section
      id="unsupported-deductions"
      aria-labelledby={headingId}
      className="rounded-xl border bg-card p-6 text-card-foreground shadow-sm"
    >
      <h3 id={headingId} className="mb-4 text-base font-semibold">
        Unsupported deductions
      </h3>
      <form onSubmit={handleSubmit} className="flex flex-col gap-4 sm:max-w-sm">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={effectiveFromId}>Effective from</Label>
          <Input
            id={effectiveFromId}
            type="date"
            required
            value={effectiveFrom}
            onChange={(event) => {
              setEffectiveFrom(event.target.value);
              edited();
            }}
          />
        </div>

        <fieldset
          role="radiogroup"
          aria-label={STATUS_QUESTION}
          {...fieldErrorProps(fieldError, 'status', errorId)}
          className="flex flex-col gap-2"
        >
          <legend className="mb-1 text-sm font-medium">{STATUS_QUESTION}</legend>
          <label className="flex w-fit items-center gap-2 text-sm">
            <input
              type="radio"
              name={`unsupported-deduction-status-${employmentId}`}
              className="size-4 accent-primary"
              checked={status === 'confirmed_none'}
              onChange={() => {
                setStatus('confirmed_none');
                setKinds(new Set());
                edited();
              }}
            />
            No
          </label>
          <label className="flex w-fit items-center gap-2 text-sm">
            <input
              type="radio"
              name={`unsupported-deduction-status-${employmentId}`}
              className="size-4 accent-primary"
              checked={status === 'present'}
              onChange={() => {
                setStatus('present');
                edited();
              }}
            />
            Yes
          </label>
        </fieldset>

        {status === 'present' && (
          <fieldset
            {...fieldErrorProps(fieldError, 'kinds', errorId)}
            className="flex flex-col gap-2"
          >
            <legend className="mb-1 text-sm font-medium">Which kinds?</legend>
            {KINDS.map((kind) => (
              <label key={kind.code} className="flex w-fit items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  className="size-4 accent-primary"
                  checked={kinds.has(kind.code)}
                  onChange={() => toggleKind(kind.code)}
                />
                {kind.label}
              </label>
            ))}
          </fieldset>
        )}

        <div className="flex flex-col gap-1.5">
          <Label htmlFor={reasonId}>Reason</Label>
          <Input
            id={reasonId}
            type="text"
            required
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
        <Button type="submit" disabled={declareStatus.isPending} className="self-start">
          {declareStatus.isPending ? 'Saving…' : 'Save unsupported deductions'}
        </Button>
      </form>

      {/* Same reason as `PriorEmploymentForm`'s: no route reads a
          declaration back, so this states only what this screen has done,
          and never calls an unanswered question a "no". */}
      {saved === null && (
        <p className="mt-3 text-sm text-muted-foreground">
          Nothing has been declared from this screen. A date with no declaration counts as unknown,
          not as “none”.
        </p>
      )}
      <p role="status" className="mt-3 text-sm text-success">
        {saved ?? ''}
      </p>
    </section>
  );
}
