// One member's earnings editor on a payroll run's own screen (issue #65,
// §0.22/§0.31). `PUT .../members/{em}/earnings` replaces the member's whole
// Earnings list, so this form always sends every line it holds, not just
// the one an Operator just typed — editing twice this way can never leave a
// stale line behind (issue #65's own first acceptance criterion).
//
// The whole list this form sends is every `taxableAllowance` line it holds,
// and that is the whole of what a member's Earnings can be: `basicPay` is
// derived from CompensationTerms, `set_run_earnings` refuses a request
// carrying one, and nothing therefore ever stores one for a member to read
// back (`crates/payroll-app/src/payroll_run.rs`'s own `set_run_earnings`).
// Echoing a `basicPay` line back into the `PUT` would not preserve it — it
// would guarantee the save was refused.
//
// This form does no arithmetic, previews no PAYE and recomputes no net pay
// when an allowance is typed (§0's Further Notes) — it sends amounts and
// shows whatever the next Calculate or reload says. Its own success also
// flags this member's currently-shown `Figures` stale (`PayrollRun.tsx`'s
// own `onChanged`, issue #88): they were true of the last Calculate, and
// this form just changed what the next one will see. Saving the same lines
// again is not a change and therefore does not raise a false stale warning.

import { type SubmitEvent, useId, useState } from 'react';
import { Plus, X } from 'lucide-react';
import { ApiError } from '../../api/client';
import { sharedFactRefusalMessage } from '../../api/refusal';
import type { EarningLineDto } from '../../api/types';
import { formatCents, parseCentsInput } from '../../money';
import { useSetRunEarnings } from '../../payrollRuns/usePayrollRuns';
import { Button } from '../../components/ui/button';
import { Input } from '../../components/ui/input';
import { Label } from '../../components/ui/label';
import { ValidationError } from '../../components/states/ValidationError';

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

/** An amount from the wire, guarded the same way every money display on
 * this screen is (`PayrollRun.tsx`'s own `centsText`) — `Money` on the wire
 * is never negative or unsafe, but a display that trusted that absolutely
 * would blank the whole form the one time it was wrong. Kept as the plain
 * decimal `formatCents` gives, never `moneyDisplayText`'s `N$1,234.56`: this
 * fills an *editable* input, and `parseCentsInput` cannot read a comma or a
 * currency symbol back out of it. */
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
  onChanged,
}: {
  payrollRunId: string;
  employmentId: string;
  earnings: EarningLineDto[];
  onChanged?: () => void;
}) {
  const setEarnings = useSetRunEarnings(payrollRunId);

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

    // The whole list, every time (issue #65's own first acceptance
    // criterion): `PUT` replaces what is stored, so a line an Operator
    // removed is gone precisely because this body does not carry it.
    const request: EarningLineDto[] = amountsCents.map((amountCents) => ({
      kind: 'taxableAllowance' as const,
      amountCents,
    }));
    const changed =
      request.length !== earnings.length ||
      request.some(
        (line, index) =>
          line.kind !== earnings[index]?.kind || line.amountCents !== earnings[index]?.amountCents,
      );

    try {
      await setEarnings.mutateAsync({ employmentId, earnings: request });
      setSaved(true);
      if (changed) {
        onChanged?.();
      }
    } catch (caught) {
      setError(earningsFailureMessage(caught));
    }
  }

  return (
    <form onSubmit={(event) => void handleSubmit(event)} className="flex flex-col gap-3">
      {allowances.length === 0 ? (
        <p className="text-sm text-muted-foreground">No taxable allowances on this line.</p>
      ) : (
        <ul className="flex flex-col gap-2">
          {allowances.map((amount, index) => {
            const inputId = `${employmentId}-allowance-${index}`;
            const invalid = lineError !== null && lineError.index === index;
            return (
              <li key={index} className="flex flex-col gap-1">
                <div className="flex items-end gap-2">
                  <div className="flex flex-col gap-1.5">
                    <Label htmlFor={inputId}>Taxable allowance</Label>
                    <Input
                      id={inputId}
                      type="text"
                      inputMode="decimal"
                      placeholder="0.00"
                      className="w-32"
                      value={amount}
                      aria-invalid={invalid || undefined}
                      aria-describedby={invalid ? errorId : undefined}
                      onChange={(event) => editAllowance(index, event.target.value)}
                    />
                  </div>
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    onClick={() => removeAllowance(index)}
                  >
                    <X className="size-4" aria-hidden="true" />
                    Remove
                  </Button>
                </div>
                {invalid && <ValidationError id={errorId}>{lineError.message}</ValidationError>}
              </li>
            );
          })}
        </ul>
      )}

      <Button
        type="button"
        variant="outline"
        size="sm"
        onClick={addAllowance}
        className="self-start"
      >
        <Plus className="size-4" aria-hidden="true" />
        Add a taxable allowance
      </Button>

      {error !== null && <ValidationError>{error}</ValidationError>}

      <Button type="submit" size="sm" disabled={setEarnings.isPending} className="self-start">
        {setEarnings.isPending ? 'Saving…' : 'Save earnings'}
      </Button>
      <p role="status" className="text-sm text-success">
        {saved ? 'Earnings saved.' : ''}
      </p>
    </form>
  );
}
