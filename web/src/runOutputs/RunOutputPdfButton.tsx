// The "Download PDF" action shared by the Register and Payment Summary
// screens (issue #83): both print the same rows and totals the on-screen
// table already shows, and — unlike the batch Payslip PDF — neither needs a
// payslip's frozen particulars (parent #70's own acceptance criterion:
// "register and payment summary still work for a payroll whose particulars
// never froze"), so there is no permanent refusal to special-case here, only
// the ordinary retryable failure every download can hit.

import { ApiError, saveAs } from '../api/client';
import { requestIdOf } from '../api/refusal';
import { Button } from '../components/ui/button';
import { FailedRequestState } from '../components/states/FailedRequestState';
import { type RunPdfKind, useRunPdfDownload } from './useRunPdfDownload';

function pdfFailureMessage(caught: unknown): string {
  if (caught instanceof ApiError && caught.code === 'internal_error') {
    const requestId = requestIdOf(caught.details);
    if (requestId !== null) {
      return `We could not prepare this PDF. Try again, and quote reference ${requestId} if the problem continues.`;
    }
  }
  return 'We could not prepare this PDF. Try again.';
}

export function RunOutputPdfButton({
  payrollRunId,
  kind,
  label,
}: {
  payrollRunId: string;
  kind: RunPdfKind;
  label: string;
}) {
  const download = useRunPdfDownload(payrollRunId, kind);

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
        {download.isPending ? 'Preparing PDF…' : label}
      </Button>
      {download.isError && (
        <FailedRequestState
          message={pdfFailureMessage(download.error)}
          onRetry={() => void handleDownload()}
          retrying={download.isPending}
        />
      )}
    </div>
  );
}
