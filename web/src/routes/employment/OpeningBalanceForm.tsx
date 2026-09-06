// `POST .../opening-balance` (issue #63, §0.35). What was already paid
// before Salt took over mid-tax-year — in scope for this ticket precisely
// because the first real customer needs it: without this form, their first
// Calculate refuses with a blocker no other screen can clear.

import { type SubmitEvent, useId, useState } from 'react';
import { ApiError } from '../../api/client';
import { sharedFactRefusalMessage } from '../../api/refusal';
import type { RecordOpeningBalanceRequest } from '../../api/types';
import { useRecordOpeningBalance } from '../../employments/useEmploymentFacts';
import { formatCents, parseCentsInput } from '../../money';
import { currentTaxYearStartingYear, parseTaxYearInput } from '../../taxYear';
import { type FieldError, fieldErrorProps } from './fieldError';

function messageForRefusal(caught: unknown): string {
  const shared = sharedFactRefusalMessage(caught);
  if (shared !== null) {
    return shared;
  }

  const error = caught as ApiError;
  switch (error.code) {
    case 'salt_coverage_start_not_a_period_end':
      return 'This date must be the last day of one of this employer’s pay periods.';

    case 'salt_coverage_start_outside_tax_year':
      return 'That date falls outside the tax year entered.';

    case 'salt_coverage_start_before_employment_is_payable': {
      const firstPayable = (error.details as { firstPayablePeriodEnd?: unknown } | null)
        ?.firstPayablePeriodEnd;
      return typeof firstPayable === 'string'
        ? `That date is before this employee could first be paid, on ${firstPayable}.`
        : 'That date is before this employee could first be paid.';
    }

    case 'opening_balance_figures_over_an_empty_covered_span':
      return 'There is no earlier period for these figures to cover. Leave both amounts at 0.00, or choose a later date.';

    case 'opening_balance_frozen_by_finalization':
      return 'Payroll for this tax year has already been finalized for this employee, so this can no longer be changed here.';

    default:
      return 'Something went wrong. Please try again.';
  }
}

/** The fields this form validates for shape, and so the fields one of its
 * own complaints can be about. */
type Field = 'taxYear' | 'figures';

export function OpeningBalanceForm({ employmentId }: { employmentId: string }) {
  const recordOpeningBalance = useRecordOpeningBalance(employmentId);
  const [taxYear, setTaxYear] = useState(() => String(currentTaxYearStartingYear()));
  const [saltCoverageStart, setSaltCoverageStart] = useState('');
  const [priorTaxableRemuneration, setPriorTaxableRemuneration] = useState('0.00');
  const [priorPaye, setPriorPaye] = useState('0.00');
  const [fieldError, setFieldError] = useState<FieldError<Field> | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const taxYearId = useId();
  const saltCoverageStartId = useId();
  const errorId = useId();

  // Clears the confirmation as well: it quotes the date and both amounts,
  // so leaving it up beside changed fields would describe a balance nobody
  // recorded.
  function edited() {
    setFieldError(null);
    setError(null);
    setSaved(null);
  }

  async function handleSubmit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    if (recordOpeningBalance.isPending) {
      return;
    }

    setError(null);
    setSaved(null);

    const priorTaxableRemunerationCents = parseCentsInput(priorTaxableRemuneration);
    const priorPayeCents = parseCentsInput(priorPaye);
    if (priorTaxableRemunerationCents === null || priorPayeCents === null) {
      setFieldError({
        field: 'figures',
        message:
          'Enter both amounts as non-negative values with no more than two decimal places, e.g. 0.00. Very large amounts are not supported.',
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
    setFieldError(null);

    const request: RecordOpeningBalanceRequest = {
      taxYear: taxYearNumber,
      saltCoverageStart,
      priorTaxableRemunerationCents,
      priorPayeCents,
    };

    try {
      await recordOpeningBalance.mutateAsync(request);
      setSaved(
        `Recorded: Salt starts with the pay period ending ${saltCoverageStart}; earlier periods carry prior taxable remuneration ${formatCents(priorTaxableRemunerationCents)} and prior PAYE ${formatCents(priorPayeCents)}.`,
      );
    } catch (caught) {
      setError(messageForRefusal(caught));
    }
  }

  return (
    <section>
      <h3>Opening balance</h3>
      <p>
        Only needed when Salt takes over part-way through a tax year. Choose the first pay period
        Salt will process; the amounts cover earlier periods in that tax year.
      </p>
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
        <div>
          <label htmlFor={saltCoverageStartId}>First Salt pay period (end date)</label>
          <input
            id={saltCoverageStartId}
            type="date"
            required
            value={saltCoverageStart}
            onChange={(event) => {
              setSaltCoverageStart(event.target.value);
              edited();
            }}
          />
        </div>
        <div>
          <label htmlFor={`${taxYearId}-taxable`}>Prior taxable remuneration</label>
          <input
            id={`${taxYearId}-taxable`}
            type="text"
            inputMode="decimal"
            required
            {...fieldErrorProps(fieldError, 'figures', errorId)}
            value={priorTaxableRemuneration}
            onChange={(event) => {
              setPriorTaxableRemuneration(event.target.value);
              edited();
            }}
          />
        </div>
        <div>
          <label htmlFor={`${taxYearId}-paye`}>Prior PAYE withheld</label>
          <input
            id={`${taxYearId}-paye`}
            type="text"
            inputMode="decimal"
            required
            {...fieldErrorProps(fieldError, 'figures', errorId)}
            value={priorPaye}
            onChange={(event) => {
              setPriorPaye(event.target.value);
              edited();
            }}
          />
        </div>
        {(fieldError !== null || error !== null) && (
          <p id={errorId} role="alert">
            {fieldError?.message ?? error}
          </p>
        )}
        <button type="submit" disabled={recordOpeningBalance.isPending}>
          {recordOpeningBalance.isPending ? 'Saving…' : 'Save opening balance'}
        </button>
      </form>
      <p role="status">{saved ?? ''}</p>
    </section>
  );
}
