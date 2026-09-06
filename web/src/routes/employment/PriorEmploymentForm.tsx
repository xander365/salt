// `POST .../prior-employment` (issue #63). Whether the employee had earlier
// taxable employment this tax year.
//
// The radio group starts with nothing checked, on purpose (this ticket's own
// Deep Instructions): a declaration nobody has made yet must not look like a
// recorded "no". "No prior employment" is a choice the Operator makes, not
// this form's default.

import { type FormEvent, useId, useState } from 'react';
import { ApiError } from '../../api/client';
import { sharedFactRefusalMessage } from '../../api/refusal';
import type { DeclarePriorEmploymentRequest, PriorEmploymentStatus } from '../../api/types';
import { useDeclarePriorEmployment } from '../../employments/useEmploymentFacts';
import { formatCents, parseCentsInput } from '../../money';
import { currentTaxYearStartingYear } from '../../taxYear';

function messageForRefusal(caught: unknown): string {
  const shared = sharedFactRefusalMessage(caught);
  if (shared !== null) {
    return shared;
  }

  const error = caught as ApiError;
  switch (error.code) {
    case 'prior_employment_frozen_by_finalization':
      return 'Payroll for this tax year has already been finalized for this employee, so this can no longer be changed here.';

    default:
      return 'Something went wrong. Please try again.';
  }
}

export function PriorEmploymentForm({ employmentId }: { employmentId: string }) {
  const declarePriorEmployment = useDeclarePriorEmployment(employmentId);
  const [taxYear, setTaxYear] = useState(() => String(currentTaxYearStartingYear()));
  const [status, setStatus] = useState<PriorEmploymentStatus | null>(null);
  const [taxableRemuneration, setTaxableRemuneration] = useState('');
  const [paye, setPaye] = useState('');
  const [fieldError, setFieldError] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const taxYearId = useId();
  const errorId = useId();

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (declarePriorEmployment.isPending) {
      return;
    }

    setError(null);
    setSaved(null);

    if (status === null) {
      setFieldError('Choose whether this employee had prior employment this tax year.');
      return;
    }

    const taxYearNumber = Number(taxYear);
    let request: DeclarePriorEmploymentRequest;
    let confirmation: string;

    if (status === 'confirmed_none') {
      request = { taxYear: taxYearNumber, status };
      confirmation = `Recorded: no prior employment in tax year ${taxYear}.`;
    } else {
      const taxableRemunerationCents = parseCentsInput(taxableRemuneration);
      const payeCents = parseCentsInput(paye);
      if (taxableRemunerationCents === null || payeCents === null) {
        setFieldError('Enter both the taxable remuneration and the PAYE already withheld.');
        return;
      }
      request = { taxYear: taxYearNumber, status, taxableRemunerationCents, payeCents };
      confirmation = `Recorded: prior employment in tax year ${taxYear}, taxable remuneration ${formatCents(taxableRemunerationCents)}, PAYE ${formatCents(payeCents)}.`;
    }
    setFieldError(null);

    try {
      await declarePriorEmployment.mutateAsync(request);
      setSaved(confirmation);
    } catch (caught) {
      setError(messageForRefusal(caught));
    }
  }

  return (
    <section>
      <h3>Prior employment</h3>
      <form onSubmit={handleSubmit}>
        <div>
          <label htmlFor={taxYearId}>Tax year starting</label>
          <input
            id={taxYearId}
            type="number"
            required
            value={taxYear}
            onChange={(event) => {
              setTaxYear(event.target.value);
              setError(null);
            }}
          />
        </div>

        <fieldset>
          <legend>Did this employee have taxable employment earlier this tax year?</legend>
          <label>
            <input
              type="radio"
              name={`prior-employment-status-${employmentId}`}
              checked={status === 'confirmed_none'}
              onChange={() => {
                setStatus('confirmed_none');
                setFieldError(null);
                setError(null);
              }}
            />
            No
          </label>
          <label>
            <input
              type="radio"
              name={`prior-employment-status-${employmentId}`}
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
          <>
            <div>
              <label htmlFor={`${taxYearId}-taxable`}>Taxable remuneration so far</label>
              <input
                id={`${taxYearId}-taxable`}
                type="text"
                inputMode="decimal"
                placeholder="0.00"
                required
                value={taxableRemuneration}
                onChange={(event) => {
                  setTaxableRemuneration(event.target.value);
                  setFieldError(null);
                  setError(null);
                }}
              />
            </div>
            <div>
              <label htmlFor={`${taxYearId}-paye`}>PAYE already withheld</label>
              <input
                id={`${taxYearId}-paye`}
                type="text"
                inputMode="decimal"
                placeholder="0.00"
                required
                value={paye}
                onChange={(event) => {
                  setPaye(event.target.value);
                  setFieldError(null);
                  setError(null);
                }}
              />
            </div>
          </>
        )}

        {(fieldError !== null || error !== null) && (
          <p id={errorId} role="alert">
            {fieldError ?? error}
          </p>
        )}
        <button type="submit" disabled={declarePriorEmployment.isPending}>
          {declarePriorEmployment.isPending ? 'Saving…' : 'Save prior employment'}
        </button>
      </form>
      <p role="status">{saved ?? ''}</p>
    </section>
  );
}
