// `PUT /api/employers/{e}/particulars` (issue #71, parent #70 D-7). Owner-only
// on this screen too: `canEdit` is `false` for a PayrollOperator, and this
// component then renders the recorded particulars as plain text with a line
// stating the restriction, rather than a form that would 403 the moment it
// is submitted (§0.6, issue #71's own acceptance criteria).
//
// One screen covers both "record" and "correct" (issue #71's own scope):
// the server decides which this write is by whether a row already exists,
// so this form asks for a reason only once one does, and shows the
// divergence-acknowledgement panel only once the server actually reports
// one — there is no way to predict either from the browser alone.

import { type SubmitEvent, useId, useState } from 'react';
import { ApiError } from '../../api/client';
import { sharedFactRefusalMessage } from '../../api/refusal';
import type {
  EmployerParticularsResponse,
  PayPeriodDto,
  SetEmployerParticularsRequest,
} from '../../api/types';
import { useSetEmployerParticulars } from '../../employer/useEmployerParticulars';
import { humanDateRange } from '../../format';
import { type FieldError, fieldErrorProps } from '../employment/fieldError';
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
    case 'employer_particulars_registered_name_cannot_be_empty':
      return 'Enter the registered name.';
    case 'employer_particulars_address_line1_cannot_be_empty':
      return 'Enter the first line of the address.';
    case 'employer_particulars_city_cannot_be_empty':
      return 'Enter the city.';
    case 'employer_particulars_correction_reason_cannot_be_empty':
      return 'Enter a reason for this change.';
    default:
      return 'Something went wrong. Please try again.';
  }
}

type Field = 'registeredName' | 'addressLine1' | 'city' | 'reason';

function fieldsFrom(particulars: EmployerParticularsResponse | null) {
  return {
    registeredName: particulars?.registeredName ?? '',
    addressLine1: particulars?.addressLine1 ?? '',
    addressLine2: particulars?.addressLine2 ?? '',
    city: particulars?.city ?? '',
    postalCode: particulars?.postalCode ?? '',
    incomeTaxNumber: particulars?.incomeTaxNumber ?? '',
    socialSecurityNumber: particulars?.socialSecurityNumber ?? '',
  };
}

function ReadOnlyParticulars({ particulars }: { particulars: EmployerParticularsResponse | null }) {
  if (particulars === null) {
    return (
      <p className="text-sm text-muted-foreground">
        Nobody has recorded this Employer’s particulars yet. Only an Owner can do that.
      </p>
    );
  }

  return (
    <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-sm">
      <dt className="text-muted-foreground">Registered name</dt>
      <dd>{particulars.registeredName}</dd>
      <dt className="text-muted-foreground">Address</dt>
      <dd>
        {[
          particulars.addressLine1,
          particulars.addressLine2,
          particulars.city,
          particulars.postalCode,
        ]
          .filter((line) => line !== null && line !== '')
          .join(', ')}
      </dd>
      {particulars.incomeTaxNumber !== null && (
        <>
          <dt className="text-muted-foreground">Income tax number</dt>
          <dd>{particulars.incomeTaxNumber}</dd>
        </>
      )}
      {particulars.socialSecurityNumber !== null && (
        <>
          <dt className="text-muted-foreground">Social security number</dt>
          <dd>{particulars.socialSecurityNumber}</dd>
        </>
      )}
    </dl>
  );
}

export function EmployerParticularsForm({
  particulars,
  canEdit,
}: {
  particulars: EmployerParticularsResponse | null;
  canEdit: boolean;
}) {
  const setParticulars = useSetEmployerParticulars();
  const [fields, setFields] = useState(() => fieldsFrom(particulars));
  const [reason, setReason] = useState('');
  const [pendingAcknowledgement, setPendingAcknowledgement] = useState<PayPeriodDto[] | null>(null);
  const [fieldError, setFieldError] = useState<FieldError<Field> | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const registeredNameId = useId();
  const addressLine1Id = useId();
  const addressLine2Id = useId();
  const cityId = useId();
  const postalCodeId = useId();
  const incomeTaxNumberId = useId();
  const socialSecurityNumberId = useId();
  const reasonId = useId();
  const errorId = useId();
  const headingId = useId();

  const isCorrection = particulars !== null;

  function edited() {
    setFieldError(null);
    setError(null);
    setSaved(false);
    // A field changed after the server named a divergence, so the pending
    // acknowledgement no longer describes what is about to be sent —
    // resubmitting must ask the server again rather than reuse a stale list.
    setPendingAcknowledgement(null);
  }

  async function submit(acknowledgedDivergingPeriods: PayPeriodDto[] | null) {
    setError(null);
    setSaved(false);

    if (fields.registeredName.trim() === '') {
      setFieldError({ field: 'registeredName', message: 'Enter the registered name.' });
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

    const request: SetEmployerParticularsRequest = {
      registeredName: fields.registeredName,
      addressLine1: fields.addressLine1,
      addressLine2: fields.addressLine2 || undefined,
      city: fields.city,
      postalCode: fields.postalCode || undefined,
      incomeTaxNumber: fields.incomeTaxNumber || undefined,
      socialSecurityNumber: fields.socialSecurityNumber || undefined,
      ...(reason.trim() !== '' ? { reason } : {}),
      ...(acknowledgedDivergingPeriods !== null ? { acknowledgedDivergingPeriods } : {}),
    };

    try {
      await setParticulars.mutateAsync(request);
      setPendingAcknowledgement(null);
      // Cleared, not kept: the next correction is a different act and owes
      // its own explanation. A reason left in the box would be sent again
      // unread, and the ActionLog would carry the previous change's words
      // as this one's.
      setReason('');
      setSaved(true);
    } catch (caught) {
      if (
        caught instanceof ApiError &&
        caught.code === 'employer_master_data_divergence_not_acknowledged'
      ) {
        const details = caught.details as { divergingPeriods?: PayPeriodDto[] } | null;
        setPendingAcknowledgement(details?.divergingPeriods ?? []);
        return;
      }
      setError(messageForRefusal(caught));
    }
  }

  async function handleSubmit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    if (setParticulars.isPending) {
      return;
    }
    await submit(null);
  }

  async function handleConfirmDivergence() {
    if (setParticulars.isPending || pendingAcknowledgement === null) {
      return;
    }
    await submit(pendingAcknowledgement);
  }

  if (!canEdit) {
    return (
      <section
        aria-labelledby={headingId}
        className="rounded-xl border bg-card p-6 text-card-foreground shadow-sm"
      >
        <h3 id={headingId} className="mb-4 text-base font-semibold">
          Employer particulars
        </h3>
        <ReadOnlyParticulars particulars={particulars} />
        <p className="mt-3 text-sm text-muted-foreground">
          Only an Owner can record or correct these details.
        </p>
      </section>
    );
  }

  return (
    <section
      aria-labelledby={headingId}
      className="rounded-xl border bg-card p-6 text-card-foreground shadow-sm"
    >
      <h3 id={headingId} className="mb-4 text-base font-semibold">
        Employer particulars
      </h3>
      <form onSubmit={handleSubmit} className="flex flex-col gap-4 sm:max-w-md">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={registeredNameId}>Registered name</Label>
          <Input
            id={registeredNameId}
            type="text"
            required
            {...fieldErrorProps(fieldError, 'registeredName', errorId)}
            value={fields.registeredName}
            onChange={(event) => {
              setFields((current) => ({ ...current, registeredName: event.target.value }));
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
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={incomeTaxNumberId}>Income tax number (optional)</Label>
          <Input
            id={incomeTaxNumberId}
            type="text"
            value={fields.incomeTaxNumber}
            onChange={(event) => {
              setFields((current) => ({ ...current, incomeTaxNumber: event.target.value }));
              edited();
            }}
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor={socialSecurityNumberId}>Social security number (optional)</Label>
          <Input
            id={socialSecurityNumberId}
            type="text"
            value={fields.socialSecurityNumber}
            onChange={(event) => {
              setFields((current) => ({ ...current, socialSecurityNumber: event.target.value }));
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
              This is a warning, not a block — nothing already finalized is changed. Enter a reason
              above and confirm to save anyway.
            </p>
            <Button
              type="button"
              variant="outline"
              className="mt-2"
              disabled={setParticulars.isPending}
              onClick={() => void handleConfirmDivergence()}
            >
              {setParticulars.isPending ? 'Saving…' : 'Confirm and save'}
            </Button>
          </div>
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
