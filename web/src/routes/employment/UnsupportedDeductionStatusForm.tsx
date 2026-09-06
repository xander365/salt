// `POST .../unsupported-deductions` (issue #63). Whether the employee has
// deductions Salt does not calculate — the same unasked-vs-recorded-"none"
// rule as `PriorEmploymentForm`: nothing is checked until the Operator
// chooses.

import { type FormEvent, useId, useState } from 'react';
import { ApiError } from '../../api/client';
import { sharedFactRefusalMessage } from '../../api/refusal';
import type {
  DeclareUnsupportedDeductionStatusRequest,
  UnsupportedDeductionKindCode,
  UnsupportedDeductionStatusValue,
} from '../../api/types';
import { useDeclareUnsupportedDeductionStatus } from '../../employments/useEmploymentFacts';

const KINDS: { code: UnsupportedDeductionKindCode; label: string }[] = [
  { code: 'approved_pension_fund', label: 'Approved pension fund contribution' },
  { code: 'provident_fund', label: 'Provident fund contribution' },
  { code: 'retirement_annuity_fund', label: 'Retirement annuity fund contribution' },
  { code: 'education_policy', label: 'Education policy premium' },
];

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

export function UnsupportedDeductionStatusForm({ employmentId }: { employmentId: string }) {
  const declareStatus = useDeclareUnsupportedDeductionStatus(employmentId);
  const [effectiveFrom, setEffectiveFrom] = useState('');
  const [status, setStatus] = useState<UnsupportedDeductionStatusValue | null>(null);
  const [kinds, setKinds] = useState<Set<UnsupportedDeductionKindCode>>(new Set());
  const [reason, setReason] = useState('');
  const [fieldError, setFieldError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const effectiveFromId = useId();
  const reasonId = useId();
  const errorId = useId();

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
    setFieldError(null);
    setError(null);
  }

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (declareStatus.isPending) {
      return;
    }

    setError(null);
    setSaved(null);

    if (status === null) {
      setFieldError('Choose whether this employee has any unsupported deductions.');
      return;
    }
    if (status === 'present' && kinds.size === 0) {
      setFieldError('Choose at least one kind of deduction.');
      return;
    }
    setFieldError(null);

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
          : `Recorded: unsupported deductions from ${effectiveFrom} (${[...kinds].length} kind${kinds.size === 1 ? '' : 's'}).`,
      );
    } catch (caught) {
      setError(messageForRefusal(caught));
    }
  }

  return (
    <section>
      <h3>Unsupported deductions</h3>
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

        <fieldset>
          <legend>Does this employee have any deductions Salt does not support?</legend>
          <label>
            <input
              type="radio"
              name={`unsupported-deduction-status-${employmentId}`}
              checked={status === 'confirmed_none'}
              onChange={() => {
                setStatus('confirmed_none');
                setKinds(new Set());
                setFieldError(null);
                setError(null);
              }}
            />
            No
          </label>
          <label>
            <input
              type="radio"
              name={`unsupported-deduction-status-${employmentId}`}
              checked={status === 'present'}
              onChange={() => {
                setStatus('present');
                setFieldError(null);
                setError(null);
              }}
            />
            Yes
          </label>
        </fieldset>

        {status === 'present' && (
          <fieldset>
            <legend>Which kinds?</legend>
            {KINDS.map((kind) => (
              <label key={kind.code}>
                <input
                  type="checkbox"
                  checked={kinds.has(kind.code)}
                  onChange={() => toggleKind(kind.code)}
                />
                {kind.label}
              </label>
            ))}
          </fieldset>
        )}

        <div>
          <label htmlFor={reasonId}>Reason</label>
          <input
            id={reasonId}
            type="text"
            required
            value={reason}
            onChange={(event) => {
              setReason(event.target.value);
              setError(null);
            }}
          />
        </div>

        {(fieldError !== null || error !== null) && (
          <p id={errorId} role="alert">
            {fieldError ?? error}
          </p>
        )}
        <button type="submit" disabled={declareStatus.isPending}>
          {declareStatus.isPending ? 'Saving…' : 'Save unsupported deductions'}
        </button>
      </form>
      <p role="status">{saved ?? ''}</p>
    </section>
  );
}
