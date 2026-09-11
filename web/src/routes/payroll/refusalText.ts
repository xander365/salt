// The Operator-facing sentence for a payroll run member's `refusal` (issue
// #65, §0.31). Unlike a blocker, a refusal's `code` is not limited to five
// values: `calculate_payroll_run` reports whatever `PayrollAppError`
// assembling or calculating that member's figures actually raised
// (`crates/payroll-app/src/calculate.rs`'s own module doc), which includes
// every blocker code plus every `PayrollError` `calculate` itself can raise.
// This lookup covers every code that use case can realistically reach and
// falls back to a plain, honest default for the rest — never a rendering of
// the server's own `message`, which may be reworded (§0.23).

import type { RefusalDto } from '../../api/types';
import { moneyDisplayText } from '../../money';
import { unsupportedDeductionKindLabel } from '../employment/unsupportedDeductionKinds';

function kindsFromDetails(details: unknown): string[] {
  if (typeof details !== 'object' || details === null) {
    return [];
  }
  const kinds = (details as { kinds?: unknown }).kinds;
  return Array.isArray(kinds)
    ? kinds.filter((kind): kind is string => typeof kind === 'string')
    : [];
}

function shortfallFromDetails(details: unknown): number | null {
  if (typeof details !== 'object' || details === null) {
    return null;
  }
  const shortfall = (details as { shortfallCents?: unknown }).shortfallCents;
  return typeof shortfall === 'number' && Number.isSafeInteger(shortfall) && shortfall >= 0
    ? shortfall
    : null;
}

/** The sentence an Operator reads for one member's refusal, chosen by
 * `code` alone. */
export function refusalSentence(refusal: RefusalDto): string {
  switch (refusal.code) {
    // The same five standing-fact codes a blocker carries (§0.31's own
    // guarantee: a blocker and the refusal for the same fact share a code).
    case 'prior_employment_unknown':
      return 'Prior employment has not been declared.';

    case 'prior_employment_treatment_unconfirmed':
      return 'Prior employment is declared, but Salt cannot yet confirm how to treat it.';

    case 'unsupported_deduction_status_unknown':
      return 'Unsupported deductions have not been declared.';

    case 'unsupported_deductions_present': {
      const kinds = kindsFromDetails(refusal.details).map(unsupportedDeductionKindLabel);
      return kinds.length === 0
        ? 'Has unsupported deductions.'
        : `Has unsupported deductions: ${kinds.join(', ')}.`;
    }

    case 'no_compensation_terms_in_force':
      return 'Pay has not been recorded.';

    // Codes only `calculate` itself can raise, once every standing fact is
    // in place.
    case 'compensation_terms_do_not_cover_period':
      return 'The recorded pay does not cover this pay period.';

    case 'pay_period_not_on_the_employers_schedule':
      return 'This pay period does not match the employer’s pay schedule.';

    case 'employment_does_not_overlap_period':
      return 'This employment does not overlap this pay period.';

    case 'duplicate_basic_pay_line':
      return 'More than one basic pay line was found for this member.';

    case 'prior_paye_exceeds_recalculated_liability':
      return 'The declared prior PAYE exceeds what this employment now owes for the year.';

    case 'deductions_exceed_gross_remuneration': {
      const shortfall = shortfallFromDetails(refusal.details);
      return shortfall === null
        ? 'This would take net pay below zero. Lower the deduction.'
        : `This would take net pay below zero by ${moneyDisplayText(shortfall)}. Lower the deduction.`;
    }

    case 'amount_overflow':
      return 'A figure for this member is too large for Salt to represent.';

    case 'no_paye_table_covers_date':
    case 'overlapping_paye_tables':
    case 'paye_table_does_not_cover_period':
      return 'Salt has no usable PAYE table for this pay period. Contact support.';

    case 'no_ssc_ruleset_covers_date':
    case 'overlapping_ssc_rulesets':
    case 'ssc_ruleset_does_not_cover_period':
      return 'Salt has no usable social security ruleset for this pay period. Contact support.';

    case 'wrong_tax_year_for_period':
      return 'This pay period falls in a different tax year than declared.';

    case 'employment_is_void':
      return 'This employment has been voided.';

    case 'employment_not_found':
      return 'This employment is no longer available.';

    // No fixed set of codes is guaranteed here, unlike a blocker's five — a
    // code this module does not know is still shown honestly, by name,
    // rather than hidden behind one sentence that fits everything.
    default:
      return `This member could not be calculated (${refusal.code}).`;
  }
}
