// The Operator-facing half of a payroll run's `blockers` (issue #64, §0.31).
//
// A blocker sentence is a lookup keyed by `code`, never a rendering of a
// server `message` — there isn't one to render anyway: `PayrollRunBlockerDto`
// carries only `code` and `details` (`crates/salt-server/src/payroll_error.rs`'s
// own `blocker_code_and_details`). There are exactly six codes; this module
// names all six and no others, matching the same rule the server's own Deep
// Instructions state for itself.
//
// `blockerSection` names the Employment screen section each blocker's fact
// is recorded in (`Employment.tsx`'s own four forms, each given a matching
// `id`) — the anchor a blocker's link resolves to, the "highest-value small
// detail" the issue calls out.

import type { PayrollRunBlockerCode, PayrollRunBlockerDto } from '../../api/types';
import { unsupportedDeductionKindLabel } from '../employment/unsupportedDeductionKinds';

const SECTION_FOR_CODE: Record<PayrollRunBlockerCode, string> = {
  prior_employment_unknown: 'prior-employment',
  prior_employment_treatment_unconfirmed: 'prior-employment',
  unsupported_deduction_status_unknown: 'unsupported-deductions',
  unsupported_deductions_present: 'unsupported-deductions',
  no_compensation_terms_in_force: 'compensation-terms',
  ordinary_hours_not_recorded: 'compensation-terms',
};

function kindsFromDetails(details: unknown): string[] {
  if (typeof details !== 'object' || details === null) {
    return [];
  }
  const kinds = (details as { kinds?: unknown }).kinds;
  return Array.isArray(kinds)
    ? kinds.filter((kind): kind is string => typeof kind === 'string')
    : [];
}

/** The sentence an Operator reads for one blocker, chosen by `code` alone. */
export function blockerSentence(blocker: PayrollRunBlockerDto): string {
  switch (blocker.code) {
    case 'prior_employment_unknown':
      return 'Prior employment has not been declared.';

    case 'prior_employment_treatment_unconfirmed':
      return 'Prior employment is declared, but Salt cannot yet confirm how to treat it.';

    case 'unsupported_deduction_status_unknown':
      return 'Unsupported deductions have not been declared.';

    case 'unsupported_deductions_present': {
      const kinds = kindsFromDetails(blocker.details).map(unsupportedDeductionKindLabel);
      return kinds.length === 0
        ? 'Has unsupported deductions.'
        : `Has unsupported deductions: ${kinds.join(', ')}.`;
    }

    case 'no_compensation_terms_in_force':
      return 'Pay has not been recorded.';

    case 'ordinary_hours_not_recorded':
      return 'This member has overtime, and their ordinary hours per week have not been recorded.';

    // The six codes above are the contract (§0.31); a seventh reaching the
    // browser is a version skew this screen cannot describe better than
    // this, but must still let the Operator act on by reaching the screen.
    default:
      return 'This cannot be paid yet.';
  }
}

/**
 * The Employment screen fragment (`#compensation-terms`, etc.) a blocker's
 * fix lives at, or `null` for a code this module does not know — a link to
 * a bare `#` would scroll nowhere and mean nothing, so an unknown blocker
 * links to the Employment screen itself and lets the Operator read its four
 * forms, which is still the screen that clears it.
 */
export function blockerSection(blocker: PayrollRunBlockerDto): string | null {
  return SECTION_FOR_CODE[blocker.code] ?? null;
}
