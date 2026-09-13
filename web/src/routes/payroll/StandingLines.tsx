// One member's standing pay items on a payroll run's own screen (issue
// #80, parent #70 §0, §D-6). Replaces `StandingPayItemProposals`
// (`PayrollRun.tsx`, issue #79): the source recorded beside a proposed line
// is operational information an Operator needs, but issue #80 also gives
// them two things to do about it — change or remove the line for this run
// only, with a reason, leaving the standing record untouched — and a third
// to see: every removed line, still visible with its reason.
//
// No payroll arithmetic happens here: an override or a removal is sent as
// stated, and the worksheet's own `figures` and `calculationState` (already
// retired by the server the moment either write lands) are what say whether
// the change has been priced yet.

import { useId, useState } from 'react';
import { ApiError } from '../../api/client';
import { sharedFactRefusalMessage } from '../../api/refusal';
import type {
  DeductionLineDto,
  DeductionPayLineDto,
  MedicalAidPremiumLineDto,
  PayLineDto,
  RemovedPayLineDto,
  StandingPayLineProvenanceDto,
  TaxableAllowanceLineDto,
} from '../../api/types';
import { formatCents, parseCentsInput } from '../../money';
import {
  useOverrideStandingPayLine,
  useRemoveStandingPayLine,
} from '../../payrollRuns/usePayrollRuns';
import { humanDate } from '../../format';
import { Money } from '../../components/Money';
import { Button } from '../../components/ui/button';
import { Input } from '../../components/ui/input';
import { Label } from '../../components/ui/label';
import { ValidationError } from '../../components/states/ValidationError';
import { employmentPath } from '../paths';
import { Link } from 'react-router-dom';

const MAX_LABEL_LENGTH = 100;

function standingLineFailureMessage(caught: unknown): string {
  const shared = sharedFactRefusalMessage(caught);
  if (shared !== null) {
    return shared;
  }
  const error = caught as ApiError;
  switch (error.code) {
    case 'payroll_run_already_finalized':
      return 'This payroll run has already been finalized and can no longer be changed.';
    case 'override_reason_cannot_be_empty':
      return 'Enter a reason for this change.';
    case 'pay_line_removal_reason_cannot_be_empty':
      return 'Enter a reason for removing this line.';
    case 'standing_medical_aid_premium_is_zero':
      return 'A medical aid premium of zero is not a deduction. Remove the line instead.';
    case 'standing_pay_line_is_removed':
      return 'This line was already removed for this run. Reload the page.';
    case 'standing_pay_line_already_removed':
      return 'This line was already removed for this run. Reload the page.';
    case 'standing_pay_line_not_found':
      return 'This line is no longer part of this run. Reload the page.';
    case 'override_changes_pay_line_kind':
      return 'This change must stay the same kind of line. Reload the page and try again.';
    default:
      return 'Something went wrong. Please try again.';
  }
}

/** An active standing allowance line — never `overtime`, which no
 * `StandingPayItem` can ever be (D14). Narrow the item's own instruction to
 * the same kind here as well, so an impossible wire mismatch cannot be
 * rendered as an invented zero standing amount. */
type StandingAllowanceLine = TaxableAllowanceLineDto &
  StandingPayLineProvenanceDto<TaxableAllowanceLineDto>;

/** An active standing premium line. */
type StandingPremiumLine = MedicalAidPremiumLineDto &
  StandingPayLineProvenanceDto<DeductionLineDto>;

/** The item's own current amount, whichever of the two standing kinds
 * `line` is — see `StandingAllowanceLine`'s own doc for why the allowance
 * branch needs a second, defensive kind check that can never actually fail
 * (a `StandingPayItem`'s own instruction is always the same kind as the
 * line it proposed). */
function standingAmountCents(line: StandingAllowanceLine | StandingPremiumLine): number {
  return line.standingPayLine.amountCents;
}

/** One active standing line — an allowance or a premium — with its own
 * "Change for this run" and "Remove for this run" actions. The caller
 * filters for `source: 'standing'` and excludes `overtime` before handing
 * one to this component. */
function StandingLine({
  payrollRunId,
  employmentId,
  line,
  runIsFinalized,
  onChanged,
}: {
  payrollRunId: string;
  employmentId: string;
  line: StandingAllowanceLine | StandingPremiumLine;
  runIsFinalized: boolean;
  onChanged?: () => void;
}) {
  const override = useOverrideStandingPayLine(payrollRunId);
  const remove = useRemoveStandingPayLine(payrollRunId);
  const [open, setOpen] = useState<'override' | 'remove' | null>(null);
  const [amount, setAmount] = useState(formatCents(line.amountCents));
  const [label, setLabel] = useState(line.kind === 'taxableAllowance' ? (line.label ?? '') : '');
  const [reason, setReason] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [errorField, setErrorField] = useState<'label' | 'amount' | 'reason' | null>(null);
  const amountId = useId();
  const labelId = useId();
  const reasonId = useId();
  const errorId = useId();

  function startOverride() {
    setAmount(formatCents(line.amountCents));
    setLabel(line.kind === 'taxableAllowance' ? (line.label ?? '') : '');
    setReason('');
    setError(null);
    setErrorField(null);
    setOpen('override');
  }

  function startRemove() {
    setReason('');
    setError(null);
    setErrorField(null);
    setOpen('remove');
  }

  async function handleOverride() {
    const cents = parseCentsInput(amount);
    if (cents === null) {
      setError('Enter a non-negative amount with no more than two decimal places, e.g. 500.00.');
      setErrorField('amount');
      return;
    }
    const trimmedLabel = label.trim();
    if (line.kind === 'taxableAllowance' && trimmedLabel.length === 0) {
      setError('Enter a label for this allowance.');
      setErrorField('label');
      return;
    }
    if (line.kind === 'taxableAllowance' && Array.from(trimmedLabel).length > MAX_LABEL_LENGTH) {
      setError(`Use ${MAX_LABEL_LENGTH} characters or fewer for the allowance label.`);
      setErrorField('label');
      return;
    }
    if (reason.trim().length === 0) {
      setError('Enter a reason for this change.');
      setErrorField('reason');
      return;
    }
    try {
      await override.mutateAsync({
        employmentId,
        standingPayItemId: line.standingPayItemId,
        line:
          line.kind === 'taxableAllowance'
            ? { kind: 'taxableAllowance', amountCents: cents, label: trimmedLabel }
            : { kind: 'medicalAidPremium', amountCents: cents },
        reason: reason.trim(),
      });
      setOpen(null);
      onChanged?.();
    } catch (caught) {
      setError(standingLineFailureMessage(caught));
      setErrorField(null);
    }
  }

  async function handleRemove() {
    if (reason.trim().length === 0) {
      setError('Enter a reason for removing this line.');
      setErrorField('reason');
      return;
    }
    try {
      await remove.mutateAsync({
        employmentId,
        standingPayItemId: line.standingPayItemId,
        reason: reason.trim(),
      });
      setOpen(null);
      onChanged?.();
    } catch (caught) {
      setError(standingLineFailureMessage(caught));
      setErrorField(null);
    }
  }

  const lineLabel =
    line.kind === 'taxableAllowance'
      ? `Taxable allowance — ${line.label ?? 'unlabelled'}`
      : 'Medical aid premium';

  return (
    <li className="flex flex-col gap-2">
      <div className="flex flex-wrap items-baseline gap-1.5">
        <span>
          {lineLabel} <Money cents={line.amountCents} />
        </span>
        {line.overrideReason !== undefined ? (
          <span className="text-muted-foreground">
            · Changed for this run only: {line.overrideReason}. Standing amount{' '}
            <Money cents={standingAmountCents(line)} />.
          </span>
        ) : (
          <span className="text-muted-foreground">
            · Standing since{' '}
            <span className="text-foreground">{humanDate(line.standingEffectiveFrom)}</span>
          </span>
        )}
      </div>

      {!runIsFinalized && open === null && (
        <div className="flex gap-2">
          <Button type="button" variant="outline" size="sm" onClick={startOverride}>
            Change for this run
          </Button>
          <Button type="button" variant="outline" size="sm" onClick={startRemove}>
            Remove for this run
          </Button>
        </div>
      )}

      {open === 'override' && (
        <div className="flex flex-col gap-2 rounded-md border p-3">
          <div className="flex flex-col gap-2 sm:flex-row sm:items-end">
            {line.kind === 'taxableAllowance' && (
              <div className="flex flex-col gap-1.5">
                <Label htmlFor={labelId}>Label</Label>
                <Input
                  id={labelId}
                  type="text"
                  className="w-48"
                  value={label}
                  aria-invalid={errorField === 'label' || undefined}
                  aria-describedby={errorField === 'label' ? errorId : undefined}
                  onChange={(event) => setLabel(event.target.value)}
                />
              </div>
            )}
            <div className="flex flex-col gap-1.5">
              <Label htmlFor={amountId}>Amount for this run</Label>
              <Input
                id={amountId}
                type="text"
                inputMode="decimal"
                className="w-32"
                value={amount}
                aria-invalid={errorField === 'amount' || undefined}
                aria-describedby={errorField === 'amount' ? errorId : undefined}
                onChange={(event) => setAmount(event.target.value)}
              />
            </div>
          </div>
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={reasonId}>Reason</Label>
            <Input
              id={reasonId}
              type="text"
              value={reason}
              aria-invalid={errorField === 'reason' || undefined}
              aria-describedby={errorField === 'reason' ? errorId : undefined}
              onChange={(event) => setReason(event.target.value)}
            />
          </div>
          {error !== null && <ValidationError id={errorId}>{error}</ValidationError>}
          <div className="flex gap-2">
            <Button
              type="button"
              size="sm"
              onClick={() => void handleOverride()}
              disabled={override.isPending}
            >
              {override.isPending ? 'Saving…' : 'Save change'}
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => setOpen(null)}
              disabled={override.isPending}
            >
              Cancel
            </Button>
          </div>
        </div>
      )}

      {open === 'remove' && (
        <div className="flex flex-col gap-2 rounded-md border p-3">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={reasonId}>Reason</Label>
            <Input
              id={reasonId}
              type="text"
              value={reason}
              aria-invalid={errorField === 'reason' || undefined}
              aria-describedby={errorField === 'reason' ? errorId : undefined}
              onChange={(event) => setReason(event.target.value)}
            />
          </div>
          {error !== null && <ValidationError id={errorId}>{error}</ValidationError>}
          <div className="flex gap-2">
            <Button
              type="button"
              variant="destructive"
              size="sm"
              onClick={() => void handleRemove()}
              disabled={remove.isPending}
            >
              {remove.isPending ? 'Removing…' : 'Remove'}
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => setOpen(null)}
              disabled={remove.isPending}
            >
              Cancel
            </Button>
          </div>
        </div>
      )}
    </li>
  );
}

export function StandingLines({
  employerId,
  payrollRunId,
  employmentId,
  earnings,
  deductions,
  removedPayLines,
  runIsFinalized,
  onChanged,
}: {
  employerId: string;
  payrollRunId: string;
  employmentId: string;
  earnings: PayLineDto[];
  deductions: DeductionPayLineDto[];
  removedPayLines: RemovedPayLineDto[];
  runIsFinalized: boolean;
  onChanged?: () => void;
}) {
  const standingEarnings: StandingAllowanceLine[] = earnings.filter(
    (line) => line.source === 'standing' && line.kind === 'taxableAllowance',
  );
  const standingDeductions: StandingPremiumLine[] = deductions.filter(
    (line) => line.source === 'standing',
  );

  if (
    standingEarnings.length === 0 &&
    standingDeductions.length === 0 &&
    removedPayLines.length === 0
  ) {
    return null;
  }

  return (
    <section aria-label="Standing pay items" className="flex flex-col gap-3 text-sm">
      <h4 className="font-medium">Standing pay items</h4>
      <ul className="flex flex-col gap-3 text-muted-foreground">
        {standingEarnings.map((line) => (
          <StandingLine
            key={line.standingPayItemId}
            payrollRunId={payrollRunId}
            employmentId={employmentId}
            line={line}
            runIsFinalized={runIsFinalized}
            onChanged={onChanged}
          />
        ))}
        {standingDeductions.map((line) => (
          <StandingLine
            key={line.standingPayItemId}
            payrollRunId={payrollRunId}
            employmentId={employmentId}
            line={line}
            runIsFinalized={runIsFinalized}
            onChanged={onChanged}
          />
        ))}
        {removedPayLines.map((line) => {
          // No `StandingPayItem` is ever `overtime` (D14), so a removed
          // line never is either — this is defensive narrowing, not a case
          // that is expected to be reached.
          if (line.kind === 'overtime') {
            return null;
          }
          return (
            <li key={line.standingPayItemId} className="flex flex-col gap-0.5">
              <span>
                {line.kind === 'taxableAllowance'
                  ? `Taxable allowance — ${line.label ?? 'unlabelled'}`
                  : 'Medical aid premium'}{' '}
                <Money cents={line.amountCents} /> · Standing since{' '}
                <span className="text-foreground">{humanDate(line.standingEffectiveFrom)}</span>
              </span>
              {line.overrideReason !== undefined && (
                <span>Changed for this run only: {line.overrideReason}.</span>
              )}
              <span>Removed for this run only: {line.removedReason}. Not paid.</span>
            </li>
          );
        })}
      </ul>
      {!runIsFinalized && (
        <p className="text-sm text-muted-foreground">
          To make a change permanent, end the item on the{' '}
          <Link
            to={`${employmentPath(employerId, employmentId)}#standing-pay-items`}
            className="font-medium text-primary hover:underline"
          >
            Employment screen
          </Link>{' '}
          and record a new one.
        </p>
      )}
    </section>
  );
}
