// The Employer and Person particulars frozen onto this `FinalizedPayroll`
// (issue #73, D22): shown exactly as they stood when this payroll
// finalized, never the Employer's or Person's *current* particulars — a
// later correction of either must not change what this screen shows for a
// payroll already committed to (ADR-0004).
//
// `employerParticulars` and `personParticulars` are both `null` only for a
// payroll finalized before issue #73 shipped: nothing was ever frozen for
// it, and that absence is shown here rather than silently falling back to
// live particulars, which is exactly the rewrite-history bug this ticket
// closes.

import type { FinalizedPayrollDetailResponse } from '../api/types';

function joinedAddress(parts: (string | null)[]): string {
  return parts.filter((line) => line !== null && line !== '').join(', ');
}

export function FrozenParticulars({
  employerParticulars,
  personParticulars,
}: {
  employerParticulars: FinalizedPayrollDetailResponse['employerParticulars'];
  personParticulars: FinalizedPayrollDetailResponse['personParticulars'];
}) {
  if (employerParticulars === null && personParticulars === null) {
    return (
      <section
        aria-label="Particulars"
        className="rounded-xl border bg-card p-6 text-card-foreground shadow-sm"
      >
        <h3 className="text-base font-semibold">Particulars</h3>
        <p className="mt-2 text-sm text-muted-foreground">
          This payroll was finalized before Salt started freezing particulars onto each payslip, so
          none are recorded here.
        </p>
      </section>
    );
  }

  const employerAddress =
    employerParticulars === null
      ? ''
      : joinedAddress([
          employerParticulars.addressLine1,
          employerParticulars.addressLine2,
          employerParticulars.city,
          employerParticulars.postalCode,
        ]);
  const personAddress =
    personParticulars === null
      ? ''
      : joinedAddress([
          personParticulars.addressLine1,
          personParticulars.addressLine2,
          personParticulars.city,
          personParticulars.postalCode,
        ]);

  return (
    <section
      aria-label="Particulars"
      className="rounded-xl border bg-card p-6 text-card-foreground shadow-sm"
    >
      <h3 className="text-base font-semibold">Particulars, as printed on this payslip</h3>
      <div className="mt-4 grid gap-6 sm:grid-cols-2">
        <div>
          <h4 className="text-sm font-medium text-muted-foreground">Employer</h4>
          {employerParticulars === null ? (
            <p className="mt-2 text-sm text-muted-foreground">
              Nobody had recorded the Employer&rsquo;s particulars when this payroll finalized.
            </p>
          ) : (
            <dl className="mt-2 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-sm">
              <dt className="text-muted-foreground">Registered name</dt>
              <dd>{employerParticulars.registeredName}</dd>
              {employerAddress !== '' && (
                <>
                  <dt className="text-muted-foreground">Address</dt>
                  <dd>{employerAddress}</dd>
                </>
              )}
              {employerParticulars.incomeTaxNumber !== null && (
                <>
                  <dt className="text-muted-foreground">Income tax number</dt>
                  <dd>{employerParticulars.incomeTaxNumber}</dd>
                </>
              )}
              {employerParticulars.socialSecurityNumber !== null && (
                <>
                  <dt className="text-muted-foreground">Social security number</dt>
                  <dd>{employerParticulars.socialSecurityNumber}</dd>
                </>
              )}
            </dl>
          )}
        </div>

        <div>
          <h4 className="text-sm font-medium text-muted-foreground">Employee</h4>
          {personParticulars === null ? (
            <p className="mt-2 text-sm text-muted-foreground">Nothing recorded.</p>
          ) : (
            <dl className="mt-2 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-sm">
              <dt className="text-muted-foreground">Full name</dt>
              <dd>{personParticulars.fullName}</dd>
              {personParticulars.identityNumber !== null && (
                <>
                  <dt className="text-muted-foreground">Identity number</dt>
                  <dd>{personParticulars.identityNumber}</dd>
                </>
              )}
              {personAddress !== '' && (
                <>
                  <dt className="text-muted-foreground">Address</dt>
                  <dd>{personAddress}</dd>
                </>
              )}
            </dl>
          )}
        </div>
      </div>
    </section>
  );
}
