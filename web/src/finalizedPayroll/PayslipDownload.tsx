// The Payslip download action (issue #82): an Operator downloads one
// employee's Payslip as a real PDF, from the finalized run screen and from
// each finalized payroll. `usePayslipDownload` calls the render-on-demand
// endpoint; everything here is only ever about presenting the loading,
// failure and success states around that one request — the PDF itself is
// never inspected or held onto beyond handing it to the browser's own save.

import { CircleAlert } from 'lucide-react';
import { ApiError, saveAs } from '../api/client';
import { missingParticularsPhrase } from '../api/payslipParticulars';
import { sharedFactRefusalMessage } from '../api/refusal';
import { usePayslipDownload } from './usePayslipDownload';
import { Button } from '../components/ui/button';
import { FailedRequestState } from '../components/states/FailedRequestState';

function downloadFailureMessage(caught: unknown): string {
  if (caught instanceof ApiError) {
    switch (caught.code) {
      // Parent #70 D-9: a payroll finalized before Salt froze what a Payslip
      // must print is refused, naming what is missing. Salt never fills the
      // gap from today's records.
      case 'payslip_particulars_not_frozen': {
        const what = missingParticularsPhrase(caught.details);
        return `This payroll was finalized before Salt recorded ${what}, so it cannot produce a payslip. Salt does not fill them in from today's records.`;
      }

      case 'finalized_payroll_not_found':
        return 'This finalized payroll is no longer available to you. Reload the page.';
    }
  }

  return sharedFactRefusalMessage(caught) ?? 'We could not prepare this payslip. Try again.';
}

function isPermanentRefusal(caught: unknown): boolean {
  return caught instanceof ApiError && caught.code === 'payslip_particulars_not_frozen';
}

export function PayslipDownload({
  finalizedPayrollId,
  personName,
}: {
  finalizedPayrollId: string;
  /** Names the button when several are on one screen, so each has its own
   * accessible name. */
  personName?: string;
}) {
  const download = usePayslipDownload(finalizedPayrollId);

  async function handleDownload() {
    try {
      const { blob, filename } = await download.mutateAsync();
      saveAs(blob, filename);
    } catch {
      // The failure is already carried on `download.isError`/`download.error`
      // — nothing further to do here than stop the unhandled rejection.
    }
  }

  const idleLabel =
    personName === undefined ? 'Download payslip' : `Download payslip for ${personName}`;

  return (
    <div className="flex flex-col items-start gap-2">
      <Button
        type="button"
        variant="outline"
        disabled={download.isPending}
        onClick={() => void handleDownload()}
      >
        {download.isPending ? 'Preparing payslip…' : idleLabel}
      </Button>
      {/* A refusal is a fact about this payroll, so it offers no retry: the
          same request can only be refused again. */}
      {download.isError && isPermanentRefusal(download.error) && (
        <p role="alert" className="flex items-start gap-1.5 text-sm text-destructive">
          <CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
          <span>{downloadFailureMessage(download.error)}</span>
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
