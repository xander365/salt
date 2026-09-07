// `POST .../prior-employment` (issue #63). Whether the employee had earlier
// taxable employment this tax year.
//
// The radio group starts with nothing checked, on purpose (this ticket's own
// Deep Instructions): a declaration nobody has made yet must not look like a
// recorded "no". "No prior employment" is a choice the Operator makes, not
// this form's default. The line under the form says the same thing in words,
// because an unchecked radio is an absence and an absence is easy to miss.

import { type SubmitEvent, useId, useState } from 'react';
import { ApiError } from '../../api/client';
import { sharedFactRefusalMessage } from '../../api/refusal';
import type { DeclarePriorEmploymentRequest, PriorEmploymentStatus } from '../../api/types';
import { useDeclarePriorEmployment } from '../../employments/useEmploymentFacts';
import { type FieldError, fieldErrorProps } from './fieldError';
import { formatCents, parseCentsInput } from '../../money';
import { currentTaxYearStartingYear, parseTaxYearInput } from '../../taxYear';

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

/** The fields this form validates for shape, and so the fields one of its
 * own complaints can be about. */
type Field = 'taxYear' | 'status' | 'figures';

/** Named once and used twice: `role="radiogroup"` replaces the native
 * `fieldset` mapping that would otherwise take the group's name from its
 * `legend`, so the question has to be stated as an `aria-label` too. */
const STATUS_QUESTION = 'Did this employee have taxable employment earlier this tax year?';

export function PriorEmploymentForm({ employmentId }: { employmentId: string }) {
  const declarePriorEmployment = useDeclarePriorEmployment(employmentId);
  const [taxYear, setTaxYear] = useState(() => String(currentTaxYearStartingYear()));
  const [status, setStatus] = useState<PriorEmploymentStatus | null>(null);
  const [taxableRemuneration, setTaxableRemuneration] = useState('');
  const [paye, setPaye] = useState('');
  const [fieldError, setFieldError] = useState<FieldError<Field> | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const taxYearId = useId();
  const errorId = useId();
  const headingId = useId();

  // Every field this form's confirmation quotes clears it: a sentence
  // reading "no prior employment" beside a radio group now saying "yes"
  // would describe a declaration nobody made.
  function edited() {
    setFieldError(null);
    setError(null);
    setSaved(null);
  }

  async function handleSubmit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    if (declarePriorEmployment.isPending) {
      return;
    }

    setError(null);
    setSaved(null);

    if (status === null) {
      setFieldError({
        field: 'status',
        message: 'Choose whether this employee had prior employment this tax year.',
      });
      return;
    }

    const taxYearNumber = parseTaxYearInput(taxYear);
    if (taxYearNumber === null) {
      setFieldError({
        field: 'taxYear',
        message: 'Enter the tax year as a four-digit year, e.g. 2026.',
      });
      return;
    }

    let request: DeclarePriorEmploymentRequest;
    let confirmation: string;

    if (status === 'confirmed_none') {
      request = { taxYear: taxYearNumber, status };
      confirmation = `Recorded: no prior employment in tax year ${taxYear}.`;
    } else {
      const taxableRemunerationCents = parseCentsInput(taxableRemuneration);
      const payeCents = parseCentsInput(paye);
      if (taxableRemunerationCents === null || payeCents === null) {
        setFieldError({
          field: 'figures',
          message:
            'Enter both amounts as non-negative values with no more than two decimal places. Very large amounts are not supported.',
        });
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
    // Named so a payroll run's `prior_employment_unknown` and
    // `prior_employment_treatment_unconfirmed` blockers can link straight
    // here (issue #64's own Deep Instructions).
    <section id="prior-employment" aria-labelledby={headingId}>
      <h3 id={headingId}>Prior employment</h3>
      <form onSubmit={handleSubmit}>
        <div>
          <label htmlFor={taxYearId}>Tax year starting</label>
          <input
            id={taxYearId}
            type="number"
            required
            min={1000}
            max={9999}
            step={1}
            {...fieldErrorProps(fieldError, 'taxYear', errorId)}
            value={taxYear}
            onChange={(event) => {
              setTaxYear(event.target.value);
              edited();
            }}
          />
        </div>

        <fieldset
          role="radiogroup"
          aria-label={STATUS_QUESTION}
          {...fieldErrorProps(fieldError, 'status', errorId)}
        >
          <legend>{STATUS_QUESTION}</legend>
          <label>
            <input
              type="radio"
              name={`prior-employment-status-${employmentId}`}
              checked={status === 'confirmed_none'}
              onChange={() => {
                setStatus('confirmed_none');
                edited();
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
                edited();
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
                {...fieldErrorProps(fieldError, 'figures', errorId)}
                value={taxableRemuneration}
                onChange={(event) => {
                  setTaxableRemuneration(event.target.value);
                  edited();
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
                {...fieldErrorProps(fieldError, 'figures', errorId)}
                value={paye}
                onChange={(event) => {
                  setPaye(event.target.value);
                  edited();
                }}
              />
            </div>
          </>
        )}

        {(fieldError !== null || error !== null) && (
          <p id={errorId} role="alert">
            {fieldError?.message ?? error}
          </p>
        )}
        <button type="submit" disabled={declarePriorEmployment.isPending}>
          {declarePriorEmployment.isPending ? 'Saving…' : 'Save prior employment'}
        </button>
      </form>

      {/* Nothing here reads the declaration back — no route does (§0.22),
          and inventing one is out of this spec. So this states exactly what
          it knows: that this screen has recorded nothing. It never says
          "none", because a question nobody answered is not an answer. */}
      {saved === null && (
        <p>
          Nothing has been declared from this screen. A tax year with no declaration counts as
          unknown, not as “none”.
        </p>
      )}
      {/* Mounted whether or not there is anything to say, so a screen reader
          announces the confirmation as a change to a region already there. */}
      <p role="status">{saved ?? ''}</p>
    </section>
  );
}
