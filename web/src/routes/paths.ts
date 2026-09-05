// Every authorized route carries `:employerId` (issue #61, §0.30's browser
// half). Built here rather than by string-interpolating a path at each call
// site, so a screen cannot link to an Employer-scoped URL that has lost the
// Employer.

export function employerPath(employerId: string): string {
  return `/app/employers/${encodeURIComponent(employerId)}`;
}

export function peoplePath(employerId: string): string {
  return `${employerPath(employerId)}/people`;
}
