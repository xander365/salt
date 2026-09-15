// The Payslip download action (issue #82 review follow-up): an Operator
// downloads one employee's Payslip as a real PDF straight from the
// finalized-payroll screen. `usePayslipDownload` calls the render-on-demand
// endpoint; everything here is only ever about presenting the loading,
// failure and success states around that one request — the PDF itself is
// never inspected or held onto beyond handing it to the browser's own save.

import { ApiError } from '../api/client';
import { requestIdOf, sharedFactRefusalMessage } from '../api/refusal';
import { usePayslipDownload } from './usePayslipDownload';
import { Button } from '../components/ui/button';
import { FailedRequestState } from '../components/states/FailedRequestState';

function downloadFailureMessage(caught: unknown): string {
  const shared = sharedFactRefusalMessage(caught);
  if (shared !== null) {
    return shared;
  }

  const error = caught as ApiError;
  switch (error.code) {
    // Issue #73: a row finalized before Salt began freezing the particulars
    // and template version a Payslip demands has nothing safe to print.
    case 'payslip_particulars_not_frozen':
      return 'This payroll predates payslip records and cannot produce one.';

    case 'internal_error': {
      const requestId = requestIdOf(error.details);
      return requestId === null
        ? 'We could not prepare this payslip. Try again.'
        : `We could not prepare this payslip. Try again, and quote reference ${requestId} if the problem continues.`;
    }

    default:
      return 'We could not prepare this payslip. Try again.';
  }
}

/** Hands the browser a file to save, without ever writing the bytes to a
 * URL this document keeps navigable — `URL.revokeObjectURL` runs the moment
 * the click has been dispatched, since the anchor never leaves the DOM long
 * enough for a slower revoke to race it. */
function saveAs(blob: Blob, filename: string) {
  const url = URL.createObjectURL(blob);
  const link = document.createElement('a');
  link.href = url;
  link.download = filename;
  document.body.appendChild(link);
  link.click();
  document.body.removeChild(link);
  URL.revokeObjectURL(url);
}

export function PayslipDownload({ finalizedPayrollId }: { finalizedPayrollId: string }) {
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

  return (
    <div className="flex flex-col items-start gap-2">
      <Button
        type="button"
        variant="outline"
        disabled={download.isPending}
        onClick={() => void handleDownload()}
      >
        {download.isPending ? 'Preparing payslip…' : 'Download payslip'}
      </Button>
      {download.isError && (
        <FailedRequestState
          message={downloadFailureMessage(download.error)}
          onRetry={() => void handleDownload()}
          retrying={download.isPending}
        />
      )}
    </div>
  );
}
