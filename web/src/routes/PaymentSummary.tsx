// `/app/employers/:employerId/payroll/:runId/payment-summary` (issue #83,
// parent #70 §D-10): names and net pay for a finalized run's live records
// only, how many rows were excluded as reversed, and the plain instruction
// that producing this means nobody has been paid.

import { ArrowLeft } from 'lucide-react';
import { Link, useParams } from 'react-router-dom';
import { ApiError } from '../api/client';
import { requestIdOf } from '../api/refusal';
import type { PaymentSummaryRowDto } from '../api/types';
import { useEmployerId } from '../employments/useEmployments';
import { humanDate, humanDateRange } from '../format';
import { Money } from '../components/Money';
import { NotFound } from './NotFound';
import { finalizedPayrollPath } from './paths';
import { LoadingState } from '../components/states/LoadingState';
import { EmptyState } from '../components/states/EmptyState';
import { FailedRequestState } from '../components/states/FailedRequestState';
import { RunNotFinalized } from '../runOutputs/RunNotFinalized';
import { RunOutputPdfButton } from '../runOutputs/RunOutputPdfButton';
import {
  CORRECTION_SENTENCE,
  NOT_A_BANK_FILE_SENTENCE,
  REPLACEMENT_SENTENCE,
} from '../runOutputs/paymentSummaryText';
import { usePaymentSummary } from '../runOutputs/usePaymentSummary';

function runWasNotFound(caught: unknown): boolean {
  return (
    caught instanceof ApiError &&
    ['not_found', 'employer_not_found', 'payroll_run_not_found'].includes(caught.code)
  );
}

function runNotFinalized(caught: unknown): boolean {
  return caught instanceof ApiError && caught.code === 'payroll_run_not_finalized';
}

function loadFailureMessage(caught: unknown): string {
  if (caught instanceof ApiError && caught.code === 'internal_error') {
    const requestId = requestIdOf(caught.details);
    if (requestId !== null) {
      return `We could not load this payment summary. Try again, and quote reference ${requestId} if the problem continues.`;
    }
  }
  return 'We could not load this payment summary.';
}

function SummaryRow({ employerId, row }: { employerId: string; row: PaymentSummaryRowDto }) {
  return (
    <tr className="border-b last:border-0 align-top">
      <td className="py-1.5 pr-3">
        <div className="flex flex-col gap-0.5">
          <Link
            to={finalizedPayrollPath(employerId, row.finalizedPayrollId)}
            className="font-medium text-primary underline underline-offset-2"
          >
            {row.fullName}
          </Link>
          {/* Same wording as `render_payment_summary`'s own
              `REPLACEMENT_SENTENCE`: a full net pay, never a difference
              (§D-10 acceptance criterion 4), and this row's own link stands
              in for the PDF's own "Replaces finalized payroll {id}" line. */}
          {row.replaces !== null && (
            <p className="text-xs text-muted-foreground">
              Replaces{' '}
              <Link
                to={finalizedPayrollPath(employerId, row.replaces)}
                className="font-medium text-primary underline underline-offset-2"
              >
                an earlier finalized payroll
              </Link>{' '}
              for the same period. {REPLACEMENT_SENTENCE}
            </p>
          )}
        </div>
      </td>
      <td className="money py-1.5 whitespace-nowrap">
        <Money cents={row.netPayCents} />
      </td>
    </tr>
  );
}

export function PaymentSummary() {
  const { runId } = useParams();
  if (runId === undefined) {
    throw new Error('PaymentSummary must be rendered at a route carrying :runId');
  }

  const employerId = useEmployerId();
  const summary = usePaymentSummary(runId);

  if (summary.isError && runWasNotFound(summary.error)) {
    return <NotFound />;
  }

  return (
    <main className="flex flex-col gap-6">
      <Link
        to=".."
        relative="path"
        aria-label="Back to payroll run"
        className="flex w-fit items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
      >
        <ArrowLeft className="size-4" aria-hidden="true" />
        Back to run
      </Link>

      {summary.isPending && <LoadingState label="Loading payment summary…" />}

      {summary.isError && runNotFinalized(summary.error) && <RunNotFinalized backTo=".." />}

      {summary.isError && !runNotFinalized(summary.error) && (
        <FailedRequestState
          message={loadFailureMessage(summary.error)}
          onRetry={() => void summary.refetch()}
          retrying={summary.isFetching}
        />
      )}

      {summary.isSuccess && (
        <>
          <div>
            <h2 className="text-2xl font-semibold tracking-tight">Payment summary</h2>
            <p className="text-sm text-muted-foreground">
              {humanDateRange(summary.data.period.start, summary.data.period.end)} · Pay date:{' '}
              {humanDate(summary.data.payDate)}
            </p>
          </div>

          {/* Body text, not a tooltip (§D-10 acceptance criterion 3), same
              words the PDF prints. */}
          <p className="text-sm font-medium text-foreground">{NOT_A_BANK_FILE_SENTENCE}</p>
          {summary.data.kind === 'correction' && (
            <p className="text-sm text-muted-foreground">{CORRECTION_SENTENCE}</p>
          )}

          <RunOutputPdfButton payrollRunId={runId} kind="paymentSummary" label="Download PDF" />

          {summary.data.rows.length === 0 ? (
            <EmptyState>No live record from this run to pay.</EmptyState>
          ) : (
            <div className="overflow-x-auto">
              <table className="w-full text-left text-sm">
                <caption className="sr-only">
                  Names and net pay for this run&rsquo;s live records only.
                </caption>
                <thead>
                  <tr className="border-b">
                    <th scope="col" className="py-1.5 pr-3 font-medium">
                      Name
                    </th>
                    <th scope="col" className="py-1.5 text-right font-medium">
                      Net Pay
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {summary.data.rows.map((row) => (
                    <SummaryRow key={row.finalizedPayrollId} employerId={employerId} row={row} />
                  ))}
                </tbody>
                <tfoot>
                  <tr className="border-t font-medium">
                    <th scope="row" className="py-1.5 pr-3 text-left font-medium">
                      Total net pay
                    </th>
                    <td className="money py-1.5 whitespace-nowrap">
                      <Money cents={summary.data.totalNetPayCents} />
                    </td>
                  </tr>
                </tfoot>
              </table>
            </div>
          )}

          <p className="text-sm text-muted-foreground">
            Excluded as reversed: {summary.data.excludedReversedCount}
          </p>
        </>
      )}
    </main>
  );
}
