// The Operator-facing half of `POST .../payroll-runs/{r}/finalize` (issue
// #66, §0.26/§0.28). A finalization refusal is never a rendering of the
// server's own `message` (§0.23) — this module reads only `code` and
// `details`, the same discipline `payroll/blockerText.ts` and
// `payroll/refusalText.ts` already keep for a blocker and a calculation
// refusal.

import { ApiError } from '../../api/client';
import { requestIdOf } from '../../api/refusal';

/** The three codes §0.26 names, plus the fourth refusal a Finalize raises
 * for the same underlying reason: the facts moved since the run was
 * calculated. `finalization_rebuild_refused` is not a mismatch — the rebuild
 * never got far enough to compare anything, because a standing fact the
 * calculation needed has been withdrawn — but it names its Employment the
 * same way, and its recovery is the same one, because recalculating is what
 * puts the reason back on the screen as that member's own refusal (§0.25).
 *
 * The recovery is always "calculate again" and never a `force` flag, which
 * this design does not have anywhere. */
const CALCULATE_AGAIN_CODES = [
  'finalization_input_mismatch',
  'finalization_rules_mismatch',
  'finalization_calculation_mismatch',
  'finalization_rebuild_refused',
] as const;

/** Whether this refusal's own recovery is "calculate again" — the only
 * recovery Finalize ever offers (§0.26). */
export function offersCalculateAgain(caught: unknown): caught is ApiError {
  return (
    caught instanceof ApiError && (CALCULATE_AGAIN_CODES as readonly string[]).includes(caught.code)
  );
}

/** The single `employmentId` a mismatch names (`payroll_error.rs`'s own
 * `classify_payroll_app_error`: each of the three variants carries exactly
 * one, and `FinalizationRebuildRefused` carries its own alongside the nested
 * refusal). `null` for a response this module cannot read, so a screen falls
 * back to naming no one rather than showing a wrong name. */
export function mismatchEmploymentId(details: unknown): string | null {
  if (typeof details !== 'object' || details === null) {
    return null;
  }
  const employmentId = (details as { employmentId?: unknown }).employmentId;
  return typeof employmentId === 'string' ? employmentId : null;
}

export interface AlreadyFinalizedMember {
  employmentId: string;
  finalizedPayrollId: string;
}

/**
 * `payroll_run_already_finalized`'s own `details` (§0.28): `finalizedPayrollId`
 * is the direct-navigation shortcut for the case with one answer, and is
 * `null` when the run finalized more than one member, where
 * `finalizedPayrolls` is the only truthful answer
 * (`crates/salt-server/src/payroll_error.rs`'s own comment on the variant).
 */
export function alreadyFinalizedDetailsOf(details: unknown): {
  finalizedPayrollId: string | null;
  finalizedPayrolls: AlreadyFinalizedMember[];
} {
  if (typeof details !== 'object' || details === null) {
    return { finalizedPayrollId: null, finalizedPayrolls: [] };
  }
  const raw = details as { finalizedPayrollId?: unknown; finalizedPayrolls?: unknown };
  const finalizedPayrollId =
    typeof raw.finalizedPayrollId === 'string' ? raw.finalizedPayrollId : null;
  const finalizedPayrolls = Array.isArray(raw.finalizedPayrolls)
    ? raw.finalizedPayrolls.filter(
        (entry): entry is AlreadyFinalizedMember =>
          typeof entry === 'object' &&
          entry !== null &&
          typeof (entry as { employmentId?: unknown }).employmentId === 'string' &&
          typeof (entry as { finalizedPayrollId?: unknown }).finalizedPayrollId === 'string',
      )
    : [];
  return { finalizedPayrollId, finalizedPayrolls };
}

/** " for Ada Lovelace", or "" when the refusal names an Employment this
 * screen is not showing — naming no one is honest, naming the wrong person
 * is not. */
function namedMember(
  details: unknown,
  members: { employmentId: string; fullName: string }[],
): string {
  const employmentId = mismatchEmploymentId(details);
  const member = members.find((candidate) => candidate.employmentId === employmentId);
  return member === undefined ? '' : ` for ${member.fullName}`;
}

/**
 * The sentence an Operator reads when Finalize itself refuses. `null` for a
 * code a screen handles by navigating or by its own recovery panel instead
 * of an error banner (§0.28's own rule: `payroll_run_already_finalized`
 * "looks like error handling and is the opposite" — rendering it as a red
 * banner would be technically correct and a lie).
 */
export function finalizeFailureMessage(
  caught: unknown,
  members: { employmentId: string; fullName: string }[],
): string | null {
  if (!(caught instanceof ApiError)) {
    return 'Something went wrong. Please try again.';
  }

  switch (caught.code) {
    // Never an error: the screen navigates to the finalized payroll instead
    // (§0.28), or, once more than one member is affected, offers its own
    // links — either way there is nothing to say here.
    case 'payroll_run_already_finalized':
      return null;

    case 'finalization_input_mismatch':
    case 'finalization_rules_mismatch':
    case 'finalization_calculation_mismatch':
      return `The facts changed${namedMember(caught.details, members)} since this run was calculated. Calculate it again.`;

    // Not a mismatch: a standing fact this member's figures were built from
    // has been withdrawn since the run calculated, so there was nothing left
    // to compare. Recalculating is still the recovery — it puts the reason
    // back on the screen as that member's own refusal (§0.25) — and the
    // nested `details.refusalCode` is deliberately not reworded into a
    // second sentence here, because the run screen is about to show the
    // server's own answer for that member in its own words.
    case 'finalization_rebuild_refused':
      return `Salt could not rebuild the figures${namedMember(caught.details, members)} from the facts as they now stand. Calculate this run again to see why.`;

    case 'payroll_run_not_calculated':
      return 'This run has not been calculated. Calculate it before finalizing.';

    case 'internal_error': {
      const requestId = requestIdOf(caught.details);
      return requestId === null
        ? 'We could not finalize this payroll run. Please try again.'
        : `We could not finalize this payroll run. Try again, and quote reference ${requestId} if the problem continues.`;
    }

    default:
      return 'We could not finalize this payroll run. Please try again.';
  }
}
