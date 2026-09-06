// The PAYE and social security workings behind a `FinalizedPayroll`'s
// figures (issue #67, parent #59 Spec 3 of 3, §0.29). Kept behind a native
// `<details>` disclosure on purpose: the finalized-payroll screen reads
// exactly as it did before this issue while it is closed, and `<details>`/
// `<summary>` are keyboard-operable and announce their expanded/collapsed
// state to assistive technology with no ARIA of our own to get wrong.
//
// `useFinalizedPayrollTraces` is only ever enabled while `open` is true, so
// the separate traces endpoint (§0.29) is never fetched alongside the
// finalized payroll itself, only once an Operator actually opens this.
//
// Every value below is rendered exactly as the traces endpoint sent it — no
// band, rate, threshold or subtotal is re-derived here (§0.29's own
// instruction), and no raw snapshot JSON is shown. The one thing this file
// translates is `clamp`, which is a wire *code* and not a figure: the same
// rule the blocker sentences follow (§0.23), for the same reason — a code
// is the contract, the words an Operator reads are ours.

import { useState } from 'react';
import { ApiError } from '../api/client';
import type { BandContributionDto, PayeTraceDto, SscClampDto, SscTraceDto } from '../api/types';
import { requestIdOf } from '../api/refusal';
import { centsText } from '../money';
import { useFinalizedPayrollTraces } from './useFinalizedPayrollTraces';

function workingsLoadFailureMessage(caught: unknown): string {
  if (caught instanceof ApiError && caught.code === 'internal_error') {
    const requestId = requestIdOf(caught.details);
    if (requestId !== null) {
      return `We could not load these workings. Try again, and quote reference ${requestId} if the problem continues.`;
    }
  }
  return 'We could not load these workings.';
}

/**
 * `SscClamp`'s three wire strings as a sentence. `default` returns the code
 * itself rather than nothing: a fourth clamp added on the server must show
 * up on the screen as something an Operator can quote, never as a blank.
 */
function clampText(clamp: SscClampDto): string {
  switch (clamp) {
    case 'none':
      return 'None. Basic pay was charged as it stands.';
    case 'floor':
      return 'Floor. Basic pay was below the floor, so the floor was charged.';
    case 'ceiling':
      return 'Ceiling. Basic pay was above the ceiling, so the ceiling was charged.';
    default:
      return clamp;
  }
}

function PayeWorkings({ trace }: { trace: PayeTraceDto }) {
  return (
    <section aria-label="PAYE workings">
      <h3>PAYE</h3>
      <dl>
        <div>
          <dt>Prior year-to-date taxable remuneration</dt>
          <dd>{centsText(trace.priorTaxableRemunerationCents)}</dd>
        </div>
        <div>
          <dt>Prior year-to-date PAYE</dt>
          <dd>{centsText(trace.priorPayeCents)}</dd>
        </div>
        <div>
          <dt>This period&rsquo;s taxable remuneration</dt>
          <dd>{centsText(trace.thisPeriodTaxableRemunerationCents)}</dd>
        </div>
        <div>
          <dt>Year-to-date taxable remuneration</dt>
          <dd>{centsText(trace.yearToDateTaxableRemunerationCents)}</dd>
        </div>
        <div>
          {/* Not cents-exact, and not rounded: the exact tax owed on the
              year to date before the period's PAYE is taken from it. Shown
              as the server's own decimal string, digit for digit. */}
          <dt>Year-to-date tax owed (exact, unrounded)</dt>
          <dd>{trace.yearToDateTaxOwed}</dd>
        </div>
        <div>
          {/* `PeriodsElapsed` is a position in the TaxYear, not a count of
              periods worked — saying so here is what stops the number being
              misread years later. */}
          <dt>Period position in the tax year</dt>
          <dd>{trace.periodsElapsed}</dd>
        </div>
      </dl>

      <h4>Bands applied</h4>
      {trace.bandsApplied.length === 0 ? (
        <p>No tax band was reached.</p>
      ) : (
        <table>
          <caption>Each PAYE band the year-to-date taxable remuneration reached.</caption>
          <thead>
            <tr>
              <th scope="col">Threshold</th>
              <th scope="col">Rate</th>
              <th scope="col">Tax</th>
            </tr>
          </thead>
          <tbody>
            {trace.bandsApplied.map((band: BandContributionDto, index: number) => (
              // The bands carry no id, and the server sends them in the
              // order they were walked, so their position is their identity.
              <tr key={index}>
                <td>{band.threshold}</td>
                <td>{band.rate}</td>
                <td>{band.tax}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}

function SscWorkings({ heading, trace }: { heading: string; trace: SscTraceDto }) {
  return (
    <section aria-label={`${heading} workings`}>
      <h3>{heading}</h3>
      <dl>
        <div>
          <dt>Basic pay</dt>
          <dd>{centsText(trace.basicPayCents)}</dd>
        </div>
        <div>
          <dt>Base charged</dt>
          <dd>{centsText(trace.baseCents)}</dd>
        </div>
        <div>
          <dt>Floor or ceiling applied</dt>
          <dd>{clampText(trace.clamp)}</dd>
        </div>
        <div>
          <dt>Rate</dt>
          <dd>{trace.rate}</dd>
        </div>
        <div>
          <dt>Floor</dt>
          <dd>{centsText(trace.floorCents)}</dd>
        </div>
        <div>
          <dt>Ceiling</dt>
          <dd>{centsText(trace.ceilingCents)}</dd>
        </div>
      </dl>
    </section>
  );
}

export function Workings({ finalizedPayrollId }: { finalizedPayrollId: string }) {
  const [open, setOpen] = useState(false);
  const traces = useFinalizedPayrollTraces(finalizedPayrollId, open);

  return (
    <details open={open} onToggle={(event) => setOpen(event.currentTarget.open)}>
      <summary>How PAYE and social security were worked out</summary>

      {/* `role="status"` because this appears in answer to the Operator's
          own click, long after the page settled: without it a screen reader
          reaches an empty disclosure and is told nothing is coming. */}
      {open && traces.isPending && <p role="status">Loading…</p>}

      {open && traces.isError && (
        <p role="alert">
          {workingsLoadFailureMessage(traces.error)}{' '}
          <button type="button" onClick={() => void traces.refetch()}>
            Try again
          </button>
        </p>
      )}

      {traces.isSuccess && (
        <>
          <PayeWorkings trace={traces.data.paye} />
          <SscWorkings heading="Employee social security" trace={traces.data.employeeSsc} />
          <SscWorkings heading="Employer social security" trace={traces.data.employerSsc} />
        </>
      )}
    </details>
  );
}
