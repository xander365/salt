// `POST .../compensation-terms` (issue #63). What an Employment is paid and
// from when. No acknowledgement-of-divergence UI: that only matters once an
// Employment already has finalized payroll a new row would disagree with,
// which cannot happen the first time pay is recorded — the scenario this
// screen exists for.

import { type FormEvent, useId, useState } from 'react';
import { ApiError } from '../../api/client';
import { sharedFactRefusalMessage } from '../../api/refusal';
import type { RecordCompensationTermsRequest } from '../../api/types';
import { useRecordCompensationTerms } from '../../employments/useEmploymentFacts';
import { formatCents, parseCentsInput } from '../../money';

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

export function CompensationTermsForm({ employmentId }: { employmentId: string }) {
  const recordCompensationTerms = useRecordCompensationTerms(employmentId);
  const [effectiveFrom, setEffectiveFrom] = useState('');
  const [basicPay, setBasicPay] = useState('');
  const [fieldError, setFieldError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const effectiveFromId = useId();
  const basicPayId = useId();
  const errorId = useId();

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (recordCompensationTerms.isPending) {
      return;
    }

    setError(null);
    setSaved(null);

    const basicPayCents = parseCentsInput(basicPay);
    if (basicPayCents === null) {
      setFieldError('Enter an amount, e.g. 15000.00.');
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
    <section>
      <h3>Pay</h3>
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
              setError(null);
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
            aria-invalid={fieldError !== null}
            aria-describedby={fieldError === null ? undefined : errorId}
            value={basicPay}
            onChange={(event) => {
              setBasicPay(event.target.value);
              setFieldError(null);
              setError(null);
            }}
          />
        </div>
        {(fieldError !== null || error !== null) && (
          <p id={errorId} role="alert">
            {fieldError ?? error}
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
