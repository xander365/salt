// 409 `payroll_run_not_finalized` (issue #83): a Draft or Calculated run has
// no Register or Payment Summary yet. Shared by both screens — same fact,
// same recovery, the only link an Operator needs back to a run that has not
// reached this point yet.

import { Link } from 'react-router-dom';

export function RunNotFinalized({ backTo }: { backTo: string }) {
  return (
    <p role="status" className="text-sm text-muted-foreground">
      This payroll run has not been finalized yet, so it has no outputs.{' '}
      <Link to={backTo} className="font-medium text-primary underline underline-offset-2">
        Back to the run
      </Link>
      .
    </p>
  );
}
