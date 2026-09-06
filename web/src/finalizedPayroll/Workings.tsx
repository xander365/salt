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
// instruction), and no raw snapshot JSON is shown.

import { useState } from 'react';
import type { BandContributionDto, PayeTraceDto, SscTraceDto } from '../api/types';
import { centsText } from '../money';
import { useFinalizedPayrollTraces } from './useFinalizedPayrollTraces';

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
          <dt>Year-to-date tax owed</dt>
          <dd>{trace.yearToDateTaxOwed}</dd>
        </div>
        <div>
          <dt>Periods elapsed</dt>
          <dd>{trace.periodsElapsed}</dd>
        </div>
      </dl>

      <h4>Bands applied</h4>
      <table>
        <thead>
          <tr>
            <th scope="col">Threshold</th>
            <th scope="col">Rate</th>
            <th scope="col">Tax</th>
          </tr>
        </thead>
        <tbody>
          {trace.bandsApplied.map((band: BandContributionDto, index: number) => (
            <tr key={index}>
              <td>{band.threshold}</td>
              <td>{band.rate}</td>
              <td>{band.tax}</td>
            </tr>
          ))}
        </tbody>
      </table>
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
          <dt>Clamp</dt>
          <dd>{trace.clamp}</dd>
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

      {open && traces.isPending && <p>Loading…</p>}

      {open && traces.isError && (
        <p role="alert">
          We could not load these workings.{' '}
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
