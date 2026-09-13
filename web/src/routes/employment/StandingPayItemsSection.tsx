// `GET`/`POST .../standing-pay-items` and `POST .../standing-pay-items/{s}/end`
// (issue #79, parent #70 §D-5). An Operator records a standing taxable
// allowance or a standing medical aid premium once, from a pay period start,
// and every new Ordinary run proposes it — so the same allowance is not
// retyped twelve times a year.
//
// Ending an item states a reason and never deletes it: a run already
// proposed from it still points at it, so an ended item stays listed with
// when it ended and why. Who ended it is an actor id meant for the audit
// trail, not a name, so it is not shown here.
//
// Recording or ending an item never changes a run that already exists. A
// draft holds the proposal it was created with (§D-6), and this screen says
// so rather than let an Operator believe an open draft just changed.

import { type SubmitEvent, useEffect, useId, useRef, useState } from 'react';
import { ApiError } from '../../api/client';
import { sharedFactRefusalMessage } from '../../api/refusal';
import type { StandingPayItemDto } from '../../api/types';
import {
  useCreateStandingPayItem,
  useEndStandingPayItem,
  useStandingPayItems,
} from '../../employments/useStandingPayItems';
import { humanDate } from '../../format';
import { formatCents, parseCentsInput } from '../../money';
import { type FieldError, fieldErrorProps } from './fieldError';
import { Money } from '../../components/Money';
import { Button } from '../../components/ui/button';
import { Input } from '../../components/ui/input';
import { Label } from '../../components/ui/label';
import { Select } from '../../components/ui/select';
import { FailedRequestState } from '../../components/states/FailedRequestState';
import { LoadingState } from '../../components/states/LoadingState';
import { ValidationError } from '../../components/states/ValidationError';

type Kind = StandingPayItemDto['kind'];

const KIND_LABELS: Record<Kind, string> = {
  taxableAllowance: 'Taxable allowance',
  medicalAidPremium: 'Medical aid premium',
};

const MAX_LABEL_LENGTH = 100;

function createRefusalMessage(caught: unknown): string {
  const shared = sharedFactRefusalMessage(caught);
  if (shared !== null) {
    return shared;
  }

  const error = caught as ApiError;
  switch (error.code) {
    // The same refusal, and the same sentence, as pay and unsupported
    // deductions: every effective-dated fact starts on a period start.
    case 'effective_from_not_a_period_start': {
      const nextValid = (error.details as { nextValidEffectiveFrom?: unknown } | null)
        ?.nextValidEffectiveFrom;
      return typeof nextValid === 'string'
        ? `This must start on the first day of a pay period. The next one starts ${nextValid}.`
        : 'This must start on the first day of one of this employer’s pay periods.';
    }

    case 'standing_medical_aid_premium_is_zero':
      return 'A medical aid premium of zero is not a deduction. Enter an amount above zero.';

    default:
      return 'Something went wrong. Please try again.';
  }
}

function endRefusalMessage(caught: unknown): string {
  const shared = sharedFactRefusalMessage(caught);
  if (shared !== null) {
    return shared;
  }

  const error = caught as ApiError;
  switch (error.code) {
    case 'standing_pay_item_end_reason_cannot_be_empty':
      return 'Enter a reason for ending this item.';

    case 'standing_pay_item_already_ended':
      return 'This item has already been ended. Reload the page.';

    case 'standing_pay_item_not_found':
      return 'This item is no longer available. Reload the page.';

    default:
      return 'Something went wrong. Please try again.';
  }
}

/** The Operator's own calendar date of a server timestamp, as `YYYY-MM-DD`
 * for `humanDate`. `en-CA` is the locale whose short date is ISO-shaped. */
function localDate(timestamp: string): string {
  const date = new Date(timestamp);
  return Number.isNaN(date.getTime()) ? timestamp : date.toLocaleDateString('en-CA');
}

/** An amount from the wire, shown only when it can be shown exactly. */
function ItemAmount({ cents }: { cents: number }) {
  return Number.isSafeInteger(cents) && cents >= 0 ? <Money cents={cents} /> : <>unavailable</>;
}

function Item({ employmentId, item }: { employmentId: string; item: StandingPayItemDto }) {
  const endItem = useEndStandingPayItem(employmentId);
  const [ending, setEnding] = useState(false);
  const [reason, setReason] = useState('');
  const [error, setError] = useState<string | null>(null);
  const reasonId = useId();
  const errorId = useId();
  const reasonRef = useRef<HTMLInputElement>(null);
  const endRef = useRef<HTMLButtonElement>(null);
  const hasOpened = useRef(false);

  // Focus follows the decision, the same way Finalize's inline confirmation
  // does: onto the reason when it opens, back onto "End" when cancelled.
  useEffect(() => {
    if (ending) {
      hasOpened.current = true;
      reasonRef.current?.focus();
    } else if (hasOpened.current) {
      endRef.current?.focus();
    }
  }, [ending]);

  async function handleEnd(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    if (endItem.isPending) {
      return;
    }
    if (reason.trim() === '') {
      setError('Enter a reason for ending this item.');
      return;
    }
    setError(null);
    try {
      await endItem.mutateAsync({ standingPayItemId: item.standingPayItemId, reason });
      setEnding(false);
    } catch (caught) {
      setError(endRefusalMessage(caught));
    }
  }

  const description = (
    <>
      <span className="font-medium">{KIND_LABELS[item.kind]}</span>
      {item.kind === 'taxableAllowance' && <> — {item.label}</>}{' '}
      <ItemAmount cents={item.amountCents} />
      <span className="text-muted-foreground"> · from {humanDate(item.effectiveFrom)}</span>
    </>
  );

  if (item.ended !== null) {
    return (
      <li className="flex flex-col gap-0.5 text-sm">
        <p className="text-muted-foreground line-through decoration-muted-foreground/60">
          {description}
        </p>
        <p className="text-muted-foreground">
          Ended {humanDate(localDate(item.ended.endedAt))}: {item.ended.reason}
        </p>
      </li>
    );
  }

  return (
    <li className="flex flex-col gap-2 text-sm">
      <div className="flex flex-wrap items-center gap-2">
        <p>{description}</p>
        {!ending && (
          <Button
            type="button"
            variant="outline"
            size="sm"
            ref={endRef}
            aria-label={`End ${KIND_LABELS[item.kind].toLowerCase()}${item.kind === 'taxableAllowance' ? ` ${item.label}` : ''}`}
            onClick={() => {
              setReason('');
              setError(null);
              setEnding(true);
            }}
          >
            End
          </Button>
        )}
      </div>
      {ending && (
        <form
          onSubmit={(event) => void handleEnd(event)}
          className="flex flex-col gap-2 sm:flex-row sm:items-end"
        >
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={reasonId}>Reason for ending</Label>
            <Input
              id={reasonId}
              ref={reasonRef}
              type="text"
              className="sm:w-64"
              value={reason}
              aria-invalid={error !== null || undefined}
              aria-describedby={error !== null ? errorId : undefined}
              onChange={(event) => {
                setReason(event.target.value);
                setError(null);
              }}
            />
          </div>
          <Button type="submit" size="sm" disabled={endItem.isPending}>
            {endItem.isPending ? 'Ending…' : 'Confirm end'}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            disabled={endItem.isPending}
            onClick={() => setEnding(false)}
          >
            Cancel
          </Button>
        </form>
      )}
      {error !== null && <ValidationError id={errorId}>{error}</ValidationError>}
    </li>
  );
}

type Field = 'label' | 'amount';

function AddItemForm({ employmentId }: { employmentId: string }) {
  const createItem = useCreateStandingPayItem(employmentId);
  const [kind, setKind] = useState<Kind>('taxableAllowance');
  const [label, setLabel] = useState('');
  const [amount, setAmount] = useState('');
  const [effectiveFrom, setEffectiveFrom] = useState('');
  const [fieldError, setFieldError] = useState<FieldError<Field> | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const kindId = useId();
  const labelId = useId();
  const amountId = useId();
  const effectiveFromId = useId();
  const errorId = useId();

  // Clears the confirmation too: it quotes the kind, amount and date.
  function edited() {
    setFieldError(null);
    setError(null);
    setSaved(null);
  }

  async function handleSubmit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    if (createItem.isPending) {
      return;
    }
    setError(null);
    setSaved(null);

    const trimmedLabel = label.trim();
    if (kind === 'taxableAllowance') {
      if (trimmedLabel.length === 0) {
        setFieldError({ field: 'label', message: 'Enter a label for this allowance.' });
        return;
      }
      if (Array.from(trimmedLabel).length > MAX_LABEL_LENGTH) {
        setFieldError({
          field: 'label',
          message: `Use ${MAX_LABEL_LENGTH} characters or fewer for the label.`,
        });
        return;
      }
    }
    const cents = parseCentsInput(amount);
    if (cents === null || (kind === 'medicalAidPremium' && cents === 0)) {
      setFieldError({
        field: 'amount',
        message:
          kind === 'medicalAidPremium'
            ? 'Enter an amount above zero with no more than two decimal places, e.g. 500.00.'
            : 'Enter a non-negative amount with no more than two decimal places, e.g. 500.00.',
      });
      return;
    }
    setFieldError(null);

    try {
      await createItem.mutateAsync(
        kind === 'taxableAllowance'
          ? { kind, amountCents: cents, label: trimmedLabel, effectiveFrom }
          : { kind, amountCents: cents, effectiveFrom },
      );
      setSaved(
        kind === 'taxableAllowance'
          ? `Standing taxable allowance “${trimmedLabel}” of ${formatCents(cents)} recorded from ${effectiveFrom}.`
          : `Standing medical aid premium of ${formatCents(cents)} recorded from ${effectiveFrom}.`,
      );
      setLabel('');
      setAmount('');
    } catch (caught) {
      setError(createRefusalMessage(caught));
    }
  }

  return (
    <>
      <form
        onSubmit={(event) => void handleSubmit(event)}
        className="flex flex-col gap-4 sm:max-w-sm"
      >
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={kindId}>Kind</Label>
          <Select
            id={kindId}
            value={kind}
            onChange={(event) => {
              setKind(event.target.value as Kind);
              edited();
            }}
          >
            <option value="taxableAllowance">{KIND_LABELS.taxableAllowance}</option>
            <option value="medicalAidPremium">{KIND_LABELS.medicalAidPremium}</option>
          </Select>
        </div>
        {kind === 'taxableAllowance' && (
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={labelId}>Allowance label</Label>
            <Input
              id={labelId}
              type="text"
              placeholder="e.g. standby"
              {...fieldErrorProps(fieldError, 'label', errorId)}
              value={label}
              onChange={(event) => {
                setLabel(event.target.value);
                edited();
              }}
            />
          </div>
        )}
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={amountId}>Amount each period</Label>
          <Input
            id={amountId}
            type="text"
            inputMode="decimal"
            placeholder="0.00"
            required
            {...fieldErrorProps(fieldError, 'amount', errorId)}
            value={amount}
            onChange={(event) => {
              setAmount(event.target.value);
              edited();
            }}
          />
        </div>
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
        {(fieldError !== null || error !== null) && (
          <ValidationError id={errorId}>{fieldError?.message ?? error}</ValidationError>
        )}
        <Button type="submit" disabled={createItem.isPending} className="self-start">
          {createItem.isPending ? 'Saving…' : 'Add standing item'}
        </Button>
      </form>
      <p role="status" className="mt-3 text-sm text-success">
        {saved ?? ''}
      </p>
    </>
  );
}

export function StandingPayItemsSection({ employmentId }: { employmentId: string }) {
  const items = useStandingPayItems(employmentId);
  const headingId = useId();

  return (
    <section
      id="standing-pay-items"
      aria-labelledby={headingId}
      className="rounded-xl border bg-card p-6 text-card-foreground shadow-sm"
    >
      <h3 id={headingId} className="mb-1 text-base font-semibold">
        Standing pay items
      </h3>
      <p className="mb-4 text-sm text-muted-foreground">
        Every new payroll run proposes the items in force at the end of its pay period, at their
        full amount. A run that already exists keeps the lines it proposed.
      </p>

      {items.isPending && <LoadingState label="Loading standing pay items…" />}
      {items.isError && (
        <FailedRequestState
          message="We could not load the standing pay items."
          onRetry={() => void items.refetch()}
          retrying={items.isFetching}
        />
      )}
      {items.isSuccess &&
        (items.data.standingPayItems.length === 0 ? (
          <p className="mb-4 text-sm text-muted-foreground">No standing pay items recorded.</p>
        ) : (
          <ul className="mb-6 flex flex-col gap-3">
            {items.data.standingPayItems.map((item) => (
              <Item key={item.standingPayItemId} employmentId={employmentId} item={item} />
            ))}
          </ul>
        ))}

      <AddItemForm employmentId={employmentId} />
    </section>
  );
}
