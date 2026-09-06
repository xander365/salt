// One member's earnings editor on a payroll run's own screen (issue #65,
// §0.22/§0.31). `PUT .../members/{em}/earnings` replaces the member's whole
// Earnings list, so this form always sends every line it holds, not just
// the one an Operator just typed — editing twice this way can never leave a
// stale line behind (issue #65's own first acceptance criterion).
//
// A `basicPay` line is round-tripped read-only rather than dropped: it is
// derived from CompensationTerms and this route refuses a request that
// tries to set one (`crates/salt-server/src/payroll_runs.rs`'s own module
// doc), so this form only ever edits `taxableAllowance` lines and sends any
// other kind straight back unchanged.
//
// This form does no arithmetic, previews no PAYE and recomputes no net pay
// when an allowance is typed (§0's Further Notes) — it sends amounts and
// shows whatever the next Calculate or reload says.

import { type SubmitEvent, useId, useState } from 'react';
import { ApiError } from '../../api/client';
import { sharedFactRefusalMessage } from '../../api/refusal';
import type { EarningLineDto } from '../../api/types';
import { formatCents, parseCentsInput } from '../../money';
import { useSetRunEarnings } from '../../payrollRuns/usePayrollRuns';

function earningsFailureMessage(caught: unknown): string {
  const shared = sharedFactRefusalMessage(caught);
  if (shared !== null) {
    return shared;
  }

  const error = caught as ApiError;
  switch (error.code) {
    case 'payroll_run_not_found':
      return 'This payroll run is no longer available to you. Reload the page.';

    case 'payroll_run_already_finalized':
      return 'This payroll run has already been finalized and can no longer be changed.';

    case 'employment_not_an_active_run_member':
      return 'This member is no longer part of this run. Reload the page.';

    case 'basic_pay_cannot_be_set_as_an_earning':
      return 'Basic pay is set from Pay on the Employment screen, not here.';

    default:
      return 'Something went wrong. Please try again.';
  }
}

/** A read-only line's amount, guarded the same way every money display on
 * this screen is (`PayrollRun.tsx`'s own `earningAmountText`) — `Money` on
 * the wire is never negative or unsafe, but a display that trusted that
 * absolutely would blank the whole form the one time it was wrong. */
function safeAmountText(cents: number): string {
  return Number.isSafeInteger(cents) && cents >= 0 ? formatCents(cents) : 'unavailable';
}

interface LineError {
  index: number;
  message: string;
}

export function EarningsForm({
  payrollRunId,
  employmentId,
  earnings,
}: {
  payrollRunId: string;
  employmentId: string;
  earnings: EarningLineDto[];
}) {
  const setEarnings = useSetRunEarnings(payrollRunId);

  // The member's non-editable lines, carried through unchanged on every
  // save — never re-derived from an input, since this form has none for
  // them.
  const [otherLines] = useState(() => earnings.filter((line) => line.kind !== 'taxableAllowance'));
  const [allowances, setAllowances] = useState(() =>
    earnings
      .filter((line) => line.kind === 'taxableAllowance')
      .map((line) => safeAmountText(line.amountCents)),
  );
  const [lineError, setLineError] = useState<LineError | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const errorId = useId();

  function edited() {
    setLineError(null);
    setError(null);
    setSaved(false);
  }

  function addAllowance() {
    setAllowances((prev) => [...prev, '']);
    edited();
  }

  function removeAllowance(index: number) {
    setAllowances((prev) => prev.filter((_, candidate) => candidate !== index));
    edited();
  }

  function editAllowance(index: number, value: string) {
    setAllowances((prev) =>
      prev.map((amount, candidate) => (candidate === index ? value : amount)),
    );
    edited();
  }

  async function handleSubmit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    if (setEarnings.isPending) {
      return;
    }

    setError(null);
    setSaved(false);

    const amountsCents: number[] = [];
    for (const [index, amount] of allowances.entries()) {
      const cents = parseCentsInput(amount);
      if (cents === null) {
        setLineError({
          index,
          message: 'Enter a non-negative amount with no more than two decimal places, e.g. 500.00.',
        });
        return;
      }
      amountsCents.push(cents);
    }
    setLineError(null);

    const request: EarningLineDto[] = [
      ...otherLines,
      ...amountsCents.map((amountCents) => ({ kind: 'taxableAllowance' as const, amountCents })),
    ];

    try {
      await setEarnings.mutateAsync({ employmentId, earnings: request });
      setSaved(true);
    } catch (caught) {
      setError(earningsFailureMessage(caught));
    }
  }

  return (
    <form onSubmit={(event) => void handleSubmit(event)}>
      {otherLines.length > 0 && (
        <ul>
          {otherLines.map((line, index) => (
            <li key={`other-${index}`}>Basic pay: {safeAmountText(line.amountCents)}</li>
          ))}
        </ul>
      )}

      {allowances.length === 0 ? (
        <p>No taxable allowances on this line.</p>
      ) : (
        <ul>
          {allowances.map((amount, index) => {
            const inputId = `${employmentId}-allowance-${index}`;
            const invalid = lineError !== null && lineError.index === index;
            return (
              <li key={index}>
                <label htmlFor={inputId}>Taxable allowance</label>{' '}
                <input
                  id={inputId}
                  type="text"
                  inputMode="decimal"
                  placeholder="0.00"
                  value={amount}
                  aria-invalid={invalid || undefined}
                  aria-describedby={invalid ? errorId : undefined}
                  onChange={(event) => editAllowance(index, event.target.value)}
                />{' '}
                <button type="button" onClick={() => removeAllowance(index)}>
                  Remove
                </button>
                {invalid && (
                  <p id={errorId} role="alert">
                    {lineError.message}
                  </p>
                )}
              </li>
            );
          })}
        </ul>
      )}

      <p>
        <button type="button" onClick={addAllowance}>
          Add a taxable allowance
        </button>
      </p>

      {error !== null && <p role="alert">{error}</p>}

      <button type="submit" disabled={setEarnings.isPending}>
        {setEarnings.isPending ? 'Saving…' : 'Save earnings'}
      </button>
      <p role="status">{saved ? 'Earnings saved.' : ''}</p>
    </form>
  );
}
