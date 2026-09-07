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
import { Money } from '../components/Money';
import { FailedRequestState } from '../components/states/FailedRequestState';
import { LoadingState } from '../components/states/LoadingState';
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
    <section aria-label="PAYE workings" className="flex flex-col gap-3">
      <h3 className="text-base font-semibold">PAYE</h3>
      <dl className="grid grid-cols-2 gap-x-6 gap-y-2 sm:grid-cols-3">
        <div>
          <dt className="text-xs text-muted-foreground">Prior year-to-date taxable remuneration</dt>
          <dd className="money text-sm">
            <Money cents={trace.priorTaxableRemunerationCents} />
          </dd>
        </div>
        <div>
          <dt className="text-xs text-muted-foreground">Prior year-to-date PAYE</dt>
          <dd className="money text-sm">
            <Money cents={trace.priorPayeCents} />
          </dd>
        </div>
        <div>
          <dt className="text-xs text-muted-foreground">
            This period&rsquo;s taxable remuneration
          </dt>
          <dd className="money text-sm">
            <Money cents={trace.thisPeriodTaxableRemunerationCents} />
          </dd>
        </div>
        <div>
          <dt className="text-xs text-muted-foreground">Year-to-date taxable remuneration</dt>
          <dd className="money text-sm">
            <Money cents={trace.yearToDateTaxableRemunerationCents} />
          </dd>
        </div>
        <div>
          {/* Not cents-exact, and not rounded: the exact tax owed on the
              year to date before the period's PAYE is taken from it. Shown
              as the server's own decimal string, digit for digit. */}
          <dt className="text-xs text-muted-foreground">
            Year-to-date tax owed (exact, unrounded)
          </dt>
          <dd className="money text-sm">{trace.yearToDateTaxOwed}</dd>
        </div>
        <div>
          {/* `PeriodsElapsed` is a position in the TaxYear, not a count of
              periods worked — saying so here is what stops the number being
              misread years later. */}
          <dt className="text-xs text-muted-foreground">Period position in the tax year</dt>
          <dd className="text-sm">{trace.periodsElapsed}</dd>
        </div>
      </dl>

      <h4 className="text-sm font-semibold">Bands applied</h4>
      {trace.bandsApplied.length === 0 ? (
        <p className="text-sm text-muted-foreground">No tax band was reached.</p>
      ) : (
        <table className="w-full text-left text-sm">
          <caption className="sr-only">
            Each PAYE band the year-to-date taxable remuneration reached.
          </caption>
          <thead>
            <tr className="border-b">
              <th scope="col" className="py-1.5 font-medium">
                Threshold
              </th>
              <th scope="col" className="py-1.5 font-medium">
                Rate
              </th>
              <th scope="col" className="py-1.5 font-medium">
                Tax
              </th>
            </tr>
          </thead>
          <tbody>
            {trace.bandsApplied.map((band: BandContributionDto, index: number) => (
              // The bands carry no id, and the server sends them in the
              // order they were walked, so their position is their identity.
              <tr key={index} className="border-b last:border-0">
                <td className="py-1.5">{band.threshold}</td>
                <td className="py-1.5">{band.rate}</td>
                <td className="money py-1.5">{band.tax}</td>
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
    <section aria-label={`${heading} workings`} className="flex flex-col gap-3">
      <h3 className="text-base font-semibold">{heading}</h3>
      <dl className="grid grid-cols-2 gap-x-6 gap-y-2 sm:grid-cols-3">
        <div>
          <dt className="text-xs text-muted-foreground">Basic pay</dt>
          <dd className="money text-sm">
            <Money cents={trace.basicPayCents} />
          </dd>
        </div>
        <div>
          <dt className="text-xs text-muted-foreground">Base charged</dt>
          <dd className="money text-sm">
            <Money cents={trace.baseCents} />
          </dd>
        </div>
        <div>
          <dt className="text-xs text-muted-foreground">Floor or ceiling applied</dt>
          <dd className="text-sm">{clampText(trace.clamp)}</dd>
        </div>
        <div>
          <dt className="text-xs text-muted-foreground">Rate</dt>
          <dd className="text-sm">{trace.rate}</dd>
        </div>
        <div>
          <dt className="text-xs text-muted-foreground">Floor</dt>
          <dd className="money text-sm">
            <Money cents={trace.floorCents} />
          </dd>
        </div>
        <div>
          <dt className="text-xs text-muted-foreground">Ceiling</dt>
          <dd className="money text-sm">
            <Money cents={trace.ceilingCents} />
          </dd>
        </div>
      </dl>
    </section>
  );
}

export function Workings({ finalizedPayrollId }: { finalizedPayrollId: string }) {
  const [open, setOpen] = useState(false);
  const traces = useFinalizedPayrollTraces(finalizedPayrollId, open);

  return (
    <details
      open={open}
      onToggle={(event) => setOpen(event.currentTarget.open)}
      className="rounded-xl border bg-card p-6 text-card-foreground shadow-sm"
    >
      <summary className="cursor-pointer text-sm font-medium">
        How PAYE and social security were worked out
      </summary>

      <div className="mt-4 flex flex-col gap-6">
        {/* `role="status"` because this appears in answer to the Operator's
            own click, long after the page settled: without it a screen
            reader reaches an empty disclosure and is told nothing is
            coming. */}
        {open && traces.isPending && <LoadingState />}

        {open && traces.isError && (
          <FailedRequestState
            message={workingsLoadFailureMessage(traces.error)}
            onRetry={() => void traces.refetch()}
            retrying={traces.isFetching}
          />
        )}

        {traces.isSuccess && (
          <>
            <PayeWorkings trace={traces.data.paye} />
            <SscWorkings heading="Employee social security" trace={traces.data.employeeSsc} />
            <SscWorkings heading="Employer social security" trace={traces.data.employerSsc} />
          </>
        )}
      </div>
    </details>
  );
}
