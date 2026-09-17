// The "Download all payslips (PDF)" action (issue #83, parent #70 §D-9): one
// PDF holding every payslip a finalized run produced, in member order. Same
// shape as `PayslipDownload.tsx` (issue #82) — a mutation the Operator
// triggers on demand, never cached — but this run-level refusal reads
// differently: `payslip_particulars_not_frozen` here means the *whole
// batch* refused because of the first row in member order missing a frozen
// particular (`crates/payroll-app/src/payslip.rs`'s own contract), and the
// Register and PaymentSummary below it are unaffected, so the sentence says
// so and links to both rather than leaving the Operator stuck.

import { CircleAlert } from 'lucide-react';
import { ApiError, saveAs } from '../api/client';
import { missingParticularsPhrase } from '../api/payslipParticulars';
import { sharedFactRefusalMessage } from '../api/refusal';
import { Button } from '../components/ui/button';
import { FailedRequestState } from '../components/states/FailedRequestState';
import { Link } from 'react-router-dom';
import { paymentSummaryPath, payrollRegisterPath } from '../routes/paths';
import { useRunPdfDownload } from './useRunPdfDownload';

function downloadFailureMessage(caught: unknown): string {
  if (caught instanceof ApiError) {
    switch (caught.code) {
      case 'payslip_particulars_not_frozen': {
        const what = missingParticularsPhrase(caught.details);
        return `This run was finalized before Salt recorded ${what}, so a batch payslip PDF cannot be produced. Salt does not fill them in from today's records.`;
      }

      case 'payroll_run_not_finalized':
        return 'This payroll run has not been finalized yet.';
    }
  }

  return sharedFactRefusalMessage(caught) ?? 'We could not prepare these payslips. Try again.';
}

function isPermanentRefusal(caught: unknown): boolean {
  return (
    caught instanceof ApiError &&
    ['payslip_particulars_not_frozen', 'payroll_run_not_finalized'].includes(caught.code)
  );
}

export function RunPayslipsDownload({
  employerId,
  payrollRunId,
}: {
  employerId: string;
  payrollRunId: string;
}) {
  const download = useRunPdfDownload(payrollRunId, 'payslips');

  async function handleDownload() {
    try {
      const { blob, filename } = await download.mutateAsync();
      saveAs(blob, filename);
    } catch {
      // The failure is already carried on `download.isError`/`download.error`
      // — nothing further to do here than stop the unhandled rejection.
    }
  }

  return (
    <div className="flex flex-col items-start gap-2">
      <Button
        type="button"
        variant="outline"
        disabled={download.isPending}
        onClick={() => void handleDownload()}
      >
        {download.isPending ? 'Preparing payslips…' : 'Download all payslips (PDF)'}
      </Button>
      {/* A refusal here is a fact about this run, so it offers no retry: the
          same request can only be refused again. */}
      {download.isError && isPermanentRefusal(download.error) && (
        <p role="alert" className="flex items-start gap-1.5 text-sm text-destructive">
          <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          <span>
            {downloadFailureMessage(download.error)}{' '}
            {download.error instanceof ApiError &&
              download.error.code === 'payslip_particulars_not_frozen' && (
                <>
                  The{' '}
                  <Link
                    to={payrollRegisterPath(employerId, payrollRunId)}
                    className="font-medium underline underline-offset-2"
                  >
                    payroll register
                  </Link>{' '}
                  and{' '}
                  <Link
                    to={paymentSummaryPath(employerId, payrollRunId)}
                    className="font-medium underline underline-offset-2"
                  >
                    payment summary
                  </Link>{' '}
                  still work.
                </>
              )}
          </span>
        </p>
      )}
      {download.isError && !isPermanentRefusal(download.error) && (
        <FailedRequestState
          message={downloadFailureMessage(download.error)}
          onRetry={() => void handleDownload()}
          retrying={download.isPending}
        />
      )}
    </div>
  );
}
