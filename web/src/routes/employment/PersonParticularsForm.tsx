// `PUT /api/employers/{e}/people/{p}/particulars` and `PUT .../name` (issue
// #72, parent #70 D-7). Two independent corrections on one Person — the
// identity number and address, and the full name itself — each with its own
// submission, its own reason and its own divergence-acknowledgement panel,
// the same "no shared state between forms" rule the other four Employment
// screen forms already follow (`routes/Employment.tsx`'s own doc comment).
//
// Never Owner-only (issue #72's own acceptance criteria, D25): unlike
// `EmployerParticularsForm`, there is no `canEdit` prop and no read-only
// branch — any active member who reached the Employment screen may correct
// either fact.
//
// The ActionLog trail at the bottom is this screen's own answer to "every
// correction writes an ActionLog entry ... and the Employment screen shows
// that trail": both forms' corrections land in the one list the particulars
// read already carries, so there is nothing else here to keep in sync.

import { type SubmitEvent, useId, useState } from 'react';
import { ApiError } from '../../api/client';
import { sharedFactRefusalMessage } from '../../api/refusal';
import type {
  ActionLogEntryDto,
  CorrectPersonFullNameRequest,
  PayPeriodDto,
  PersonParticularsResponse,
  SetPersonParticularsRequest,
} from '../../api/types';
import {
  useCorrectPersonFullName,
  usePersonParticulars,
  useSetPersonParticulars,
} from '../../employments/usePersonParticulars';
import { humanDateRange } from '../../format';
import { type FieldError, fieldErrorProps } from './fieldError';
import { Button } from '../../components/ui/button';
import { Input } from '../../components/ui/input';
import { Label } from '../../components/ui/label';
import { ValidationError } from '../../components/states/ValidationError';
import { LoadingState } from '../../components/states/LoadingState';
import { FailedRequestState } from '../../components/states/FailedRequestState';

function messageForParticularsRefusal(caught: unknown): string {
  const shared = sharedFactRefusalMessage(caught);
  if (shared !== null) {
    return shared;
  }

  const error = caught as ApiError;
  switch (error.code) {
    case 'person_particulars_identity_number_cannot_be_empty':
      return 'Enter the identity number.';
    case 'person_particulars_address_line1_cannot_be_empty':
      return 'Enter the first line of the address.';
    case 'person_particulars_city_cannot_be_empty':
      return 'Enter the city.';
    case 'person_particulars_correction_reason_cannot_be_empty':
      return 'Enter a reason for this change.';
    default:
      return 'Something went wrong. Please try again.';
  }
}

function messageForNameRefusal(caught: unknown): string {
  const shared = sharedFactRefusalMessage(caught);
  if (shared !== null) {
    return shared;
  }

  const error = caught as ApiError;
  switch (error.code) {
    case 'person_full_name_cannot_be_empty':
      return 'Enter the full name.';
    case 'person_name_correction_reason_cannot_be_empty':
      return 'Enter a reason for this change.';
    default:
      return 'Something went wrong. Please try again.';
  }
}

type ParticularsField = 'identityNumber' | 'addressLine1' | 'city' | 'reason';

function DivergencePanel({
  pendingAcknowledgement,
  pending,
  onConfirm,
}: {
  pendingAcknowledgement: PayPeriodDto[];
  pending: boolean;
  onConfirm: () => void;
}) {
  return (
    <div className="rounded-md border border-warning/30 bg-warning/10 p-3 text-sm">
      <p className="font-medium">This disagrees with payroll already finalized</p>
      {pendingAcknowledgement.length === 0 ? (
        <p className="mt-1 text-muted-foreground">
          Nothing is finalized yet, but confirm to continue.
        </p>
      ) : (
        <>
          <p className="mt-1 text-muted-foreground">
            These pay periods were finalized under the old details:
          </p>
          <ul className="mt-1 list-inside list-disc">
            {pendingAcknowledgement.map((period) => (
              <li key={`${period.start}-${period.end}`}>
                {humanDateRange(period.start, period.end)}
              </li>
            ))}
          </ul>
        </>
      )}
      <p className="mt-1 text-muted-foreground">
        This is a warning, not a block — nothing already finalized is changed. Enter a reason above
        and confirm to save anyway.
      </p>
      <Button
        type="button"
        variant="outline"
        className="mt-2"
        disabled={pending}
        onClick={onConfirm}
      >
        {pending ? 'Saving…' : 'Confirm and save'}
      </Button>
    </div>
  );
}

function PersonNameForm({ personId, fullName }: { personId: string; fullName: string }) {
  const correctName = useCorrectPersonFullName(personId);
  const [name, setName] = useState(fullName);
  const [reason, setReason] = useState('');
  const [pendingAcknowledgement, setPendingAcknowledgement] = useState<PayPeriodDto[] | null>(null);
  const [fieldError, setFieldError] = useState<FieldError<'fullName' | 'reason'> | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const nameId = useId();
  const reasonId = useId();
  const errorId = useId();
  const headingId = useId();

  function edited() {
    setFieldError(null);
    setError(null);
    setSaved(false);
    setPendingAcknowledgement(null);
  }

  async function submit(acknowledgedDivergingPeriods: PayPeriodDto[] | null) {
    setError(null);
    setSaved(false);

    if (name.trim() === '') {
      setFieldError({ field: 'fullName', message: 'Enter the full name.' });
      return;
    }
    if (reason.trim() === '') {
      setFieldError({ field: 'reason', message: 'Enter a reason for this change.' });
      return;
    }
    setFieldError(null);

    const request: CorrectPersonFullNameRequest = {
      fullName: name,
      reason,
      ...(acknowledgedDivergingPeriods !== null ? { acknowledgedDivergingPeriods } : {}),
    };

    try {
      await correctName.mutateAsync(request);
      setPendingAcknowledgement(null);
      setReason('');
      setSaved(true);
    } catch (caught) {
      if (
        caught instanceof ApiError &&
        caught.code === 'person_master_data_divergence_not_acknowledged'
      ) {
        const details = caught.details as { divergingPeriods?: PayPeriodDto[] } | null;
        setPendingAcknowledgement(details?.divergingPeriods ?? []);
        return;
      }
      setError(messageForNameRefusal(caught));
    }
  }

  async function handleSubmit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    if (correctName.isPending) {
      return;
    }
    await submit(null);
  }

  return (
    <section
      aria-labelledby={headingId}
      className="rounded-xl border bg-card p-6 text-card-foreground shadow-sm"
    >
      <h3 id={headingId} className="mb-4 text-base font-semibold">
        Full name
      </h3>
      <form onSubmit={handleSubmit} className="flex flex-col gap-4 sm:max-w-md">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={nameId}>Full name</Label>
          <Input
            id={nameId}
            type="text"
            required
            {...fieldErrorProps(fieldError, 'fullName', errorId)}
            value={name}
            onChange={(event) => {
              setName(event.target.value);
              edited();
            }}
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={reasonId}>Reason for this change</Label>
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

        {pendingAcknowledgement !== null && (
          <DivergencePanel
            pendingAcknowledgement={pendingAcknowledgement}
            pending={correctName.isPending}
            onConfirm={() => void submit(pendingAcknowledgement)}
          />
        )}

        {pendingAcknowledgement === null && (
          <Button type="submit" disabled={correctName.isPending} className="self-start">
            {correctName.isPending ? 'Saving…' : 'Correct name'}
          </Button>
        )}
      </form>
      <p role="status" className="mt-3 text-sm text-success">
        {saved ? 'Name corrected.' : ''}
      </p>
    </section>
  );
}

function fieldsFrom(particulars: PersonParticularsResponse) {
  return {
    identityNumber: particulars.identityNumber ?? '',
    addressLine1: particulars.addressLine1 ?? '',
    addressLine2: particulars.addressLine2 ?? '',
    city: particulars.city ?? '',
    postalCode: particulars.postalCode ?? '',
  };
}

function PersonIdentityForm({
  personId,
  particulars,
}: {
  personId: string;
  particulars: PersonParticularsResponse;
}) {
  const setParticulars = useSetPersonParticulars(personId);
  const [fields, setFields] = useState(() => fieldsFrom(particulars));
  const [reason, setReason] = useState('');
  const [pendingAcknowledgement, setPendingAcknowledgement] = useState<PayPeriodDto[] | null>(null);
  const [fieldError, setFieldError] = useState<FieldError<ParticularsField> | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const identityNumberId = useId();
  const addressLine1Id = useId();
  const addressLine2Id = useId();
  const cityId = useId();
  const postalCodeId = useId();
  const reasonId = useId();
  const errorId = useId();
  const headingId = useId();

  const isCorrection = particulars.identityNumber !== null;

  function edited() {
    setFieldError(null);
    setError(null);
    setSaved(false);
    setPendingAcknowledgement(null);
  }

  async function submit(acknowledgedDivergingPeriods: PayPeriodDto[] | null) {
    setError(null);
    setSaved(false);

    if (fields.identityNumber.trim() === '') {
      setFieldError({ field: 'identityNumber', message: 'Enter the identity number.' });
      return;
    }
    if (fields.addressLine1.trim() === '') {
      setFieldError({ field: 'addressLine1', message: 'Enter the first line of the address.' });
      return;
    }
    if (fields.city.trim() === '') {
      setFieldError({ field: 'city', message: 'Enter the city.' });
      return;
    }
    if ((isCorrection || acknowledgedDivergingPeriods !== null) && reason.trim() === '') {
      setFieldError({ field: 'reason', message: 'Enter a reason for this change.' });
      return;
    }
    setFieldError(null);

    const request: SetPersonParticularsRequest = {
      identityNumber: fields.identityNumber,
      addressLine1: fields.addressLine1,
      addressLine2: fields.addressLine2 || undefined,
      city: fields.city,
      postalCode: fields.postalCode || undefined,
      ...(reason.trim() !== '' ? { reason } : {}),
      ...(acknowledgedDivergingPeriods !== null ? { acknowledgedDivergingPeriods } : {}),
    };

    try {
      await setParticulars.mutateAsync(request);
      setPendingAcknowledgement(null);
      setReason('');
      setSaved(true);
    } catch (caught) {
      if (
        caught instanceof ApiError &&
        caught.code === 'person_master_data_divergence_not_acknowledged'
      ) {
        const details = caught.details as { divergingPeriods?: PayPeriodDto[] } | null;
        setPendingAcknowledgement(details?.divergingPeriods ?? []);
        return;
      }
      setError(messageForParticularsRefusal(caught));
    }
  }

  async function handleSubmit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    if (setParticulars.isPending) {
      return;
    }
    await submit(null);
  }

  return (
    <section
      aria-labelledby={headingId}
      className="rounded-xl border bg-card p-6 text-card-foreground shadow-sm"
    >
      <h3 id={headingId} className="mb-4 text-base font-semibold">
        Identity number and address
      </h3>
      <form onSubmit={handleSubmit} className="flex flex-col gap-4 sm:max-w-md">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={identityNumberId}>Identity number</Label>
          <Input
            id={identityNumberId}
            type="text"
            required
            {...fieldErrorProps(fieldError, 'identityNumber', errorId)}
            value={fields.identityNumber}
            onChange={(event) => {
              setFields((current) => ({ ...current, identityNumber: event.target.value }));
              edited();
            }}
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={addressLine1Id}>Address line 1</Label>
          <Input
            id={addressLine1Id}
            type="text"
            required
            {...fieldErrorProps(fieldError, 'addressLine1', errorId)}
            value={fields.addressLine1}
            onChange={(event) => {
              setFields((current) => ({ ...current, addressLine1: event.target.value }));
              edited();
            }}
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={addressLine2Id}>Address line 2 (optional)</Label>
          <Input
            id={addressLine2Id}
            type="text"
            value={fields.addressLine2}
            onChange={(event) => {
              setFields((current) => ({ ...current, addressLine2: event.target.value }));
              edited();
            }}
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={cityId}>City</Label>
          <Input
            id={cityId}
            type="text"
            required
            {...fieldErrorProps(fieldError, 'city', errorId)}
            value={fields.city}
            onChange={(event) => {
              setFields((current) => ({ ...current, city: event.target.value }));
              edited();
            }}
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={postalCodeId}>Postal code (optional)</Label>
          <Input
            id={postalCodeId}
            type="text"
            value={fields.postalCode}
            onChange={(event) => {
              setFields((current) => ({ ...current, postalCode: event.target.value }));
              edited();
            }}
          />
        </div>

        {(isCorrection || pendingAcknowledgement !== null) && (
          <div className="flex flex-col gap-1.5">
            <Label htmlFor={reasonId}>Reason for this change</Label>
            <Input
              id={reasonId}
              type="text"
              required
              {...fieldErrorProps(fieldError, 'reason', errorId)}
              value={reason}
              onChange={(event) => {
                setReason(event.target.value);
                setFieldError(null);
                setError(null);
                setSaved(false);
              }}
            />
          </div>
        )}

        {(fieldError !== null || error !== null) && (
          <ValidationError id={errorId}>{fieldError?.message ?? error}</ValidationError>
        )}

        {pendingAcknowledgement !== null && (
          <DivergencePanel
            pendingAcknowledgement={pendingAcknowledgement}
            pending={setParticulars.isPending}
            onConfirm={() => void submit(pendingAcknowledgement)}
          />
        )}

        {pendingAcknowledgement === null && (
          <Button type="submit" disabled={setParticulars.isPending} className="self-start">
            {setParticulars.isPending ? 'Saving…' : 'Save particulars'}
          </Button>
        )}
      </form>
      <p role="status" className="mt-3 text-sm text-success">
        {saved ? 'Particulars saved.' : ''}
      </p>
    </section>
  );
}

const ACTION_TYPE_LABEL: Record<string, string> = {
  person_particulars_corrected: 'Particulars corrected',
  person_full_name_corrected: 'Name corrected',
};

function ActionLogTrail({ entries }: { entries: ActionLogEntryDto[] }) {
  if (entries.length === 0) {
    return null;
  }

  return (
    <section className="rounded-xl border bg-card p-6 text-card-foreground shadow-sm">
      <h3 className="mb-4 text-base font-semibold">Correction history</h3>
      <ol className="flex flex-col gap-3 text-sm">
        {entries.map((entry, index) => {
          const context = entry.context ?? {};
          const reason = typeof context.reason === 'string' ? context.reason : null;
          return (
            // Newest first, from the server's own ordering — an id is not
            // sent over the wire for a read-only list, so the occurrence
            // together with its position is what makes each row distinct.
            <li key={`${entry.occurredAt}-${index}`} className="border-b pb-3 last:border-b-0">
              <p className="font-medium">
                {ACTION_TYPE_LABEL[entry.actionType] ?? entry.actionType}
              </p>
              <p className="text-muted-foreground">
                {new Date(entry.occurredAt).toLocaleString()} by {entry.actor}
              </p>
              {reason !== null && <p className="mt-1">“{reason}”</p>}
            </li>
          );
        })}
      </ol>
    </section>
  );
}

export function PersonParticularsSection({ personId }: { personId: string }) {
  const particulars = usePersonParticulars(personId);

  if (particulars.isPending) {
    return <LoadingState label="Loading particulars…" />;
  }

  if (particulars.isError) {
    return (
      <FailedRequestState
        message="We could not load this Person’s particulars."
        onRetry={() => void particulars.refetch()}
        retrying={particulars.isFetching}
      />
    );
  }

  return (
    <>
      <PersonNameForm personId={personId} fullName={particulars.data.fullName} />
      <PersonIdentityForm personId={personId} particulars={particulars.data} />
      <ActionLogTrail entries={particulars.data.actionLog} />
    </>
  );
}
