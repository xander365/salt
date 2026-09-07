// `POST .../compensation-terms` (issue #63). What an Employment is paid and
// from when. No acknowledgement-of-divergence UI: that only matters once an
// Employment already has finalized payroll a new row would disagree with,
// which cannot happen the first time pay is recorded — the scenario this
// screen exists for.

import { type SubmitEvent, useId, useState } from 'react';
import { ApiError } from '../../api/client';
import { sharedFactRefusalMessage } from '../../api/refusal';
import type { RecordCompensationTermsRequest } from '../../api/types';
import { useRecordCompensationTerms } from '../../employments/useEmploymentFacts';
import { formatCents, parseCentsInput } from '../../money';
import { type FieldError, fieldErrorProps } from './fieldError';

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
        ? `Pay must start on the first day of a pay period. The next one starts ${nextValid}.`
        : 'Pay must start on the first day of one of this employer’s pay periods.';
    }

    case 'master_data_divergence_not_acknowledged':
      return 'This would change pay for a period already paid, which this screen cannot confirm. Contact support.';

    case 'compensation_terms_correction_reason_cannot_be_empty':
      return 'Enter a reason for this change.';

    default:
      return 'Something went wrong. Please try again.';
  }
}

/** The one field this form validates for shape, named the same way its
 * siblings name theirs. */
type Field = 'basicPay';

export function CompensationTermsForm({ employmentId }: { employmentId: string }) {
  const recordCompensationTerms = useRecordCompensationTerms(employmentId);
  const [effectiveFrom, setEffectiveFrom] = useState('');
  const [basicPay, setBasicPay] = useState('');
  const [fieldError, setFieldError] = useState<FieldError<Field> | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const effectiveFromId = useId();
  const basicPayId = useId();
  const errorId = useId();
  const headingId = useId();

  // Clears the confirmation as well: it quotes the amount and the date, so
  // leaving it up beside changed fields would describe pay nobody saved.
  function edited() {
    setFieldError(null);
    setError(null);
    setSaved(null);
  }

  async function handleSubmit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    if (recordCompensationTerms.isPending) {
      return;
    }

    setError(null);
    setSaved(null);

    const basicPayCents = parseCentsInput(basicPay);
    if (basicPayCents === null) {
      setFieldError({
        field: 'basicPay',
        message:
          'Enter a non-negative amount with no more than two decimal places, e.g. 15000.00. Very large amounts are not supported.',
      });
      return;
    }
    setFieldError(null);

    const request: RecordCompensationTermsRequest = { effectiveFrom, basicPayCents };

    try {
      await recordCompensationTerms.mutateAsync(request);
      setSaved(`Pay of ${formatCents(basicPayCents)} recorded from ${effectiveFrom}.`);
    } catch (caught) {
      setError(messageForRefusal(caught));
    }
  }

  return (
    // Named so a payroll run's `no_compensation_terms_in_force` blocker can
    // link straight here (issue #64's own Deep Instructions: a link is the
    // highest-value detail a blocker sentence carries).
    <section id="compensation-terms" aria-labelledby={headingId}>
      <h3 id={headingId}>Pay</h3>
      <form onSubmit={handleSubmit}>
        <div>
          <label htmlFor={effectiveFromId}>Effective from</label>
          <input
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
        <div>
          <label htmlFor={basicPayId}>Basic pay</label>
          <input
            id={basicPayId}
            type="text"
            inputMode="decimal"
            placeholder="0.00"
            required
            {...fieldErrorProps(fieldError, 'basicPay', errorId)}
            value={basicPay}
            onChange={(event) => {
              setBasicPay(event.target.value);
              edited();
            }}
          />
        </div>
        {(fieldError !== null || error !== null) && (
          <p id={errorId} role="alert">
            {fieldError?.message ?? error}
          </p>
        )}
        <button type="submit" disabled={recordCompensationTerms.isPending}>
          {recordCompensationTerms.isPending ? 'Saving…' : 'Save pay'}
        </button>
      </form>
      <p role="status">{saved ?? ''}</p>
    </section>
  );
}
