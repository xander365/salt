// The deduction kinds Salt does not calculate
// (`crates/payroll/src/unsupported_deduction.rs`), by their stable wire code
// and the label both `UnsupportedDeductionStatusForm` and a payroll run's
// `unsupported_deductions_present` blocker (issue #64) show for one — pulled
// out of the form itself so a plain function/constant module can be shared
// without breaking that component file's fast-refresh contract.
//
// The first four are deductions NamRA's brochure allows against taxable
// income. `employer_paid_medical_aid` (issue #78) is not one of those four
// at all — it is a fringe benefit whose taxable value Salt cannot compute
// (`Q-OPEN-9`) — grouped here only because both kinds block calculation the
// same way. The **employee's own** medical aid premium is not here: Salt
// supports it, as a `VoluntaryDeduction` entered on the payroll run itself.

import type { UnsupportedDeductionKindCode } from '../../api/types';

export const KINDS: { code: UnsupportedDeductionKindCode; label: string }[] = [
  { code: 'approved_pension_fund', label: 'Approved pension fund contribution' },
  { code: 'provident_fund', label: 'Provident fund contribution' },
  { code: 'retirement_annuity_fund', label: 'Retirement annuity fund contribution' },
  { code: 'education_policy', label: 'Education policy premium' },
  { code: 'employer_paid_medical_aid', label: 'Employer-paid medical aid benefit' },
];

/** The label this form shows for one kind, by its wire code. Falls back to
 * the code itself for one this list does not know, rather than hiding it. */
export function unsupportedDeductionKindLabel(code: string): string {
  const kind = KINDS.find((candidate) => candidate.code === code);
  return kind === undefined ? code : kind.label.toLowerCase();
}
