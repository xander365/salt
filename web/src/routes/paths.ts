// Every authorized route carries `:employerId` (issue #61, §0.30's browser
// half). Built here rather than by string-interpolating a path at each call
// site, so a screen cannot link to an Employer-scoped URL that has lost the
// Employer.

export function employerPath(employerId: string): string {
  return `/app/employers/${encodeURIComponent(employerId)}`;
}

/**
 * `/app/employers/:employerId/people/:employmentId`, built rather than
 * rendered as a relative `Link` (`People.tsx`'s own way of linking to it):
 * a payroll run's blocker links here from a route tree that does not share
 * this one as an ancestor, so there is no relative path to resolve against.
 */
export function employmentPath(employerId: string, employmentId: string): string {
  return `${employerPath(employerId)}/people/${encodeURIComponent(employmentId)}`;
}

/**
 * `/app/employers/:employerId/finalized/:finalizedPayrollId`, built rather
 * than rendered as a relative `Link` for the same reason
 * {@link employmentPath} is: `PayrollRun.tsx` navigates here on Finalize
 * success (§0.28), a jump between two route subtrees with no relative path
 * to resolve against.
 */
export function finalizedPayrollPath(employerId: string, finalizedPayrollId: string): string {
  return `${employerPath(employerId)}/finalized/${encodeURIComponent(finalizedPayrollId)}`;
}

/**
 * `/app/employers/:employerId/payroll/:runId/register` (issue #83): built
 * rather than rendered as a relative `Link` for the same reason {@link
 * finalizedPayrollPath} is — `PayrollRun.tsx`'s own Outputs section links
 * here from beside `payroll/:runId` in the route tree, not beneath it.
 */
export function payrollRegisterPath(employerId: string, payrollRunId: string): string {
  return `${employerPath(employerId)}/payroll/${encodeURIComponent(payrollRunId)}/register`;
}

/** `/app/employers/:employerId/payroll/:runId/payment-summary` (issue #83). */
export function paymentSummaryPath(employerId: string, payrollRunId: string): string {
  return `${employerPath(employerId)}/payroll/${encodeURIComponent(payrollRunId)}/payment-summary`;
}
