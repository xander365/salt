// One member's earnings and deductions editor on a payroll run's own screen
// (issue #65, §0.22/§0.31; overtime added by issue #76; medical aid premium
// deductions added by issue #78). `PUT .../members/{em}/pay-lines` (issue
// #77) replaces the member's whole list, so this form always sends every
// line it holds, not just the one an Operator just typed — editing twice this
// way can never leave a stale line behind (issue #65's own first acceptance
// criterion).
//
// The whole list this form sends is its labelled `taxableAllowance` lines
// followed by its `overtime` lines for `earnings`, and its `medicalAidPremium`
// lines for `deductions` — the whole of what a member's earning and voluntary
// deduction instructions can be. Basic pay is derived from CompensationTerms
// and cannot be entered here. A medical aid premium is withheld exactly as
// instructed: this form does no PAYE or social-security arithmetic, and
// Salt computes those two figures as if the deduction did not exist
// (SC-OPEN-7).
//
// An overtime line carries **hours and a multiplier, never money** (D14,
// ADR-0022): Salt derives the rate from the Employment's own pay and ordinary
// hours, which is the whole reason for typing hours rather than a figure
// worked out in a spreadsheet. The multiplier set is closed at 1.5 and 2.0
// (D31), so it is a `<select>` of two and never a free number field.
//
// This form does no arithmetic, previews no PAYE and recomputes no net pay
// when a line is typed (§0's Further Notes) — it sends inputs and shows
// whatever the next Calculate or reload says. That holds doubly for overtime:
// this file must never derive an hourly rate to preview, because the divisor
// is Salt policy (SC-OPEN-6) and the workings screen is where it is shown
// with its stamp. Its own success also flags this member's currently-shown
// `Figures` stale (`PayrollRun.tsx`'s own `onChanged`, issue #88): they were
// true of the last Calculate, and this form just changed what the next one
// will see. Saving the same lines again is not a change and therefore does
// not raise a false stale warning — and the server agrees, writing nothing
// for it (issue #77). Lines are sent without a `source`: where each line came
// from is the server's record, not something this form can claim.

import { type SubmitEvent, useId, useState } from 'react';
import { Plus, X } from 'lucide-react';
import { ApiError } from '../../api/client';
import { sharedFactRefusalMessage } from '../../api/refusal';
import type {
  DeductionLineDto,
  DeductionPayLineDto,
  EarningLineDto,
  PayLineDto,
} from '../../api/types';
import { formatCents, parseCentsInput } from '../../money';
import { useSetRunPayLines } from '../../payrollRuns/usePayrollRuns';
import { Button } from '../../components/ui/button';
import { Input } from '../../components/ui/input';
import { Label } from '../../components/ui/label';
import { Select } from '../../components/ui/select';
import { ValidationError } from '../../components/states/ValidationError';

function earningsFailureMessage(caught: unknown): string {
  const shared = sharedFactRefusalMessage(caught);
  if (shared !== null) {
    return shared;
  }

  const error = caught as ApiError;
  switch (error.code) {
    case 'payroll_run_not_found':
      return 'This payroll run is no longer available to you. Reload the page.';

    case 'payroll_run_already_finalized':
      return 'This payroll run has already been finalized and can no longer be changed.';

    case 'employment_not_an_active_run_member':
      return 'This member is no longer part of this run. Reload the page.';

    // Both of these are refusals this form validates for before sending, so
    // reaching one means the browser and the server disagree about what is
    // acceptable. Say which figure, not "something went wrong".
    case 'unsupported_overtime_multiplier':
      return 'Salt supports overtime at 1.5 and 2.0 only. Reload the page and choose one of them.';

    case 'invalid_overtime_hours':
      return 'Those overtime hours were refused. Enter hours above zero, to at most two decimal places.';

    case 'voluntary_deduction_amount_is_zero':
      return 'A medical aid premium of zero is not a deduction. Enter an amount above zero, or remove the line.';

    // Issue #80: this form only ever restates a standing line exactly as
    // it was given, so reaching this means the run's standing lines moved
    // in the time since this screen loaded them.
    case 'standing_pay_line_changed_without_override':
      return 'The standing lines on this run changed. Reload the page.';

    default:
      return 'Something went wrong. Please try again.';
  }
}

/** An amount from the wire, guarded the same way every money display on
 * this screen is (`PayrollRun.tsx`'s own `centsText`) — `Money` on the wire
 * is never negative or unsafe, but a display that trusted that absolutely
 * would blank the whole form the one time it was wrong. Kept as the plain
 * decimal `formatCents` gives, never `moneyDisplayText`'s `N$1,234.56`: this
 * fills an *editable* input, and `parseCentsInput` cannot read a comma or a
 * currency symbol back out of it. */
function safeAmountText(cents: number): string {
  return Number.isSafeInteger(cents) && cents >= 0 ? formatCents(cents) : 'unavailable';
}

/**
 * The two multipliers Salt supports, as the exact decimal strings the wire
 * uses (D31). A closed set on the server, so a closed set here: a third
 * factor is a code change on both sides, never a number an Operator types.
 *
 * Deliberately labelled by the factor itself and not by an occasion. Naming
 * one of them "Sunday" would state as fact something no verified Namibian
 * source establishes (`Q-OPEN-8`), and the occasion is what the free-text
 * label is for.
 */
const MULTIPLIERS = [
  { value: '1.5', label: '1.5 x' },
  { value: '2', label: '2.0 x' },
] as const;

/** Hours as an Operator types them: above zero, at most two decimals. The
 * same shape the server enforces, checked here so the refusal names the
 * field rather than arriving as a whole-form error. Returns the trimmed
 * string to send, or `null`. */
function parseHoursInput(raw: string): string | null {
  const trimmed = raw.trim();
  if (!/^\d+(?:\.\d{1,2})?$/.test(trimmed)) {
    return null;
  }
  return Number(trimmed) > 0 ? trimmed : null;
}

interface LineError {
  kind: 'allowance' | 'overtime' | 'medicalAidPremium';
  index: number;
  field: string;
  message: string;
}

interface AllowanceDraft {
  amount: string;
  label: string;
}

interface OvertimeDraft {
  hours: string;
  multiplier: string;
  label: string;
}

interface MedicalAidPremiumDraft {
  amount: string;
}

const MAX_LABEL_LENGTH = 100;

/** Strips a stored line back to the bare instruction the wire's write body
 * accepts — the extra fields a `PayLineDto` carries (`source`,
 * `standingPayItemId`, `overrideReason`, …) are Salt's own record, never
 * something a write body restates as its own claim. */
function toEarningLineDto(line: PayLineDto): EarningLineDto {
  return line.kind === 'taxableAllowance'
    ? { kind: 'taxableAllowance', amountCents: line.amountCents, label: line.label }
    : { kind: 'overtime', hours: line.hours, multiplier: line.multiplier, label: line.label };
}

/** `toEarningLineDto`'s counterpart for deduction lines. */
function toDeductionLineDto(line: DeductionPayLineDto): DeductionLineDto {
  return { kind: 'medicalAidPremium', amountCents: line.amountCents };
}

export function EarningsForm({
  payrollRunId,
  employmentId,
  earnings,
  deductions,
  onChanged,
}: {
  payrollRunId: string;
  employmentId: string;
  earnings: PayLineDto[];
  deductions: DeductionPayLineDto[];
  onChanged?: () => void;
}) {
  const setEarnings = useSetRunPayLines(payrollRunId);

  // Only a one-off or copied line is edited here (issue #80): a standing
  // line is changed through "Change for this run" or "Remove for this run"
  // on `StandingLines`, never by editing it in this list — the whole reason
  // being that this form's own save must restate every active standing line
  // exactly, and it can only do that by never having touched one.
  const editableEarnings = earnings.filter((line) => line.source !== 'standing');
  const editableDeductions = deductions.filter((line) => line.source !== 'standing');
  const standingEarnings = earnings.filter((line) => line.source === 'standing');
  const standingDeductions = deductions.filter((line) => line.source === 'standing');

  const [allowances, setAllowances] = useState(() =>
    editableEarnings
      .filter((line) => line.kind === 'taxableAllowance')
      .map((line) => ({
        amount: safeAmountText(line.amountCents),
        label: line.label ?? '',
      })),
  );
  const [overtimes, setOvertimes] = useState<OvertimeDraft[]>(() =>
    editableEarnings
      .filter((line) => line.kind === 'overtime')
      .map((line) => ({
        hours: line.hours,
        // An unrecognised multiplier from the wire is shown as itself rather
        // than silently corrected to one of the two: the Operator must see
        // what is stored before they change it.
        multiplier: line.multiplier,
        label: line.label ?? '',
      })),
  );
  const [medicalAidPremiums, setMedicalAidPremiums] = useState<MedicalAidPremiumDraft[]>(() =>
    editableDeductions
      .filter((line) => line.kind === 'medicalAidPremium')
      .map((line) => ({ amount: safeAmountText(line.amountCents) })),
  );
  const [lineError, setLineError] = useState<LineError | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);
  const errorId = useId();

  function edited() {
    setLineError(null);
    setError(null);
    setSaved(false);
  }

  function addAllowance() {
    setAllowances((prev) => [...prev, { amount: '', label: '' }]);
    edited();
  }

  function removeAllowance(index: number) {
    setAllowances((prev) => prev.filter((_, candidate) => candidate !== index));
    edited();
  }

  function editAllowance(index: number, field: keyof AllowanceDraft, value: string) {
    setAllowances((prev) =>
      prev.map((allowance, candidate) =>
        candidate === index ? { ...allowance, [field]: value } : allowance,
      ),
    );
    edited();
  }

  function addOvertime() {
    setOvertimes((prev) => [...prev, { hours: '', multiplier: MULTIPLIERS[0].value, label: '' }]);
    edited();
  }

  function removeOvertime(index: number) {
    setOvertimes((prev) => prev.filter((_, candidate) => candidate !== index));
    edited();
  }

  function editOvertime(index: number, field: keyof OvertimeDraft, value: string) {
    setOvertimes((prev) =>
      prev.map((overtime, candidate) =>
        candidate === index ? { ...overtime, [field]: value } : overtime,
      ),
    );
    edited();
  }

  function addMedicalAidPremium() {
    setMedicalAidPremiums((prev) => [...prev, { amount: '' }]);
    edited();
  }

  function removeMedicalAidPremium(index: number) {
    setMedicalAidPremiums((prev) => prev.filter((_, candidate) => candidate !== index));
    edited();
  }

  function editMedicalAidPremium(index: number, value: string) {
    setMedicalAidPremiums((prev) =>
      prev.map((premium, candidate) => (candidate === index ? { amount: value } : premium)),
    );
    edited();
  }

  /** Every earning line this form holds, allowances first then overtime,
   * validated. Returns `null` once it has set the error naming the
   * offending field. */
  function buildRequest(): EarningLineDto[] | null {
    const request: EarningLineDto[] = [];

    for (const [index, allowance] of allowances.entries()) {
      const label = allowance.label.trim();
      if (label.length === 0) {
        setLineError({
          kind: 'allowance',
          index,
          field: 'label',
          message: 'Enter a label for this taxable allowance.',
        });
        return null;
      }
      if (Array.from(label).length > MAX_LABEL_LENGTH) {
        setLineError({
          kind: 'allowance',
          index,
          field: 'label',
          message: `Use ${MAX_LABEL_LENGTH} characters or fewer for the allowance label.`,
        });
        return null;
      }

      const cents = parseCentsInput(allowance.amount);
      if (cents === null) {
        setLineError({
          kind: 'allowance',
          index,
          field: 'amount',
          message: 'Enter a non-negative amount with no more than two decimal places, e.g. 500.00.',
        });
        return null;
      }
      request.push({ kind: 'taxableAllowance', amountCents: cents, label });
    }

    for (const [index, overtime] of overtimes.entries()) {
      const label = overtime.label.trim();
      if (Array.from(label).length > MAX_LABEL_LENGTH) {
        setLineError({
          kind: 'overtime',
          index,
          field: 'label',
          message: `Use ${MAX_LABEL_LENGTH} characters or fewer for the overtime label.`,
        });
        return null;
      }

      const hours = parseHoursInput(overtime.hours);
      if (hours === null) {
        setLineError({
          kind: 'overtime',
          index,
          field: 'hours',
          message: 'Enter hours above zero with no more than two decimal places, e.g. 12 or 7.5.',
        });
        return null;
      }
      if (!MULTIPLIERS.some((supported) => supported.value === overtime.multiplier)) {
        setLineError({
          kind: 'overtime',
          index,
          field: 'multiplier',
          message: 'Choose 1.5 or 2.0. Salt supports no other overtime multiplier.',
        });
        return null;
      }
      request.push({
        kind: 'overtime',
        hours,
        multiplier: overtime.multiplier,
        label: label.length === 0 ? null : label,
      });
    }

    return request;
  }

  /** Every deduction line this form holds, validated. Unlike an allowance,
   * a premium must be above zero: the server refuses a zero-amount deduction
   * (`voluntary_deduction_amount_is_zero`), so this checks first and names
   * the field. Returns `null` once it has set the error naming the
   * offending field. */
  function buildDeductionsRequest(): DeductionLineDto[] | null {
    const request: DeductionLineDto[] = [];

    for (const [index, premium] of medicalAidPremiums.entries()) {
      const cents = parseCentsInput(premium.amount);
      if (cents === null || cents === 0) {
        setLineError({
          kind: 'medicalAidPremium',
          index,
          field: 'amount',
          message: 'Enter an amount above zero with no more than two decimal places, e.g. 500.00.',
        });
        return null;
      }
      request.push({ kind: 'medicalAidPremium', amountCents: cents });
    }

    return request;
  }

  async function handleSubmit(event: SubmitEvent<HTMLFormElement>) {
    event.preventDefault();
    if (setEarnings.isPending) {
      return;
    }

    setError(null);
    setSaved(false);

    const request = buildRequest();
    if (request === null) {
      return;
    }
    const deductionsRequest = buildDeductionsRequest();
    if (deductionsRequest === null) {
      return;
    }
    setLineError(null);

    // The whole list, every time (issue #65's own first acceptance
    // criterion): `PUT` replaces what is stored, so a line an Operator
    // removed is gone precisely because this body does not carry it. Every
    // active standing line is restated exactly, ahead of the edited lines
    // (issue #80, decision 2) — this form never touches one, so the
    // restatement is always byte-identical to what the server just gave it.
    const changed =
      request.length !== editableEarnings.length ||
      request.some((line, index) => !sameLine(line, editableEarnings[index])) ||
      deductionsRequest.length !== editableDeductions.length ||
      deductionsRequest.some((line, index) => !sameDeductionLine(line, editableDeductions[index]));

    try {
      await setEarnings.mutateAsync({
        employmentId,
        earnings: [...standingEarnings.map(toEarningLineDto), ...request],
        deductions: [...standingDeductions.map(toDeductionLineDto), ...deductionsRequest],
      });
      setSaved(true);
      if (changed) {
        onChanged?.();
      }
    } catch (caught) {
      setError(earningsFailureMessage(caught));
    }
  }

  return (
    <form onSubmit={(event) => void handleSubmit(event)} className="flex flex-col gap-5">
      <section aria-label="Taxable allowances" className="flex flex-col gap-3">
        {allowances.length === 0 ? (
          <p className="text-sm text-muted-foreground">No taxable allowances on this line.</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {allowances.map((allowance, index) => {
              const labelId = `${employmentId}-allowance-label-${index}`;
              const amountId = `${employmentId}-allowance-amount-${index}`;
              const at = (field: string) =>
                lineError?.kind === 'allowance' &&
                lineError.index === index &&
                lineError.field === field;
              const labelErrorId = `${errorId}-allowance-${index}-label`;
              const amountErrorId = `${errorId}-allowance-${index}-amount`;
              return (
                <li key={index} className="flex flex-col gap-1">
                  <div className="flex flex-col gap-2 sm:flex-row sm:items-end">
                    <div className="flex flex-col gap-1.5">
                      <Label htmlFor={labelId}>Allowance label</Label>
                      <Input
                        id={labelId}
                        type="text"
                        placeholder="e.g. standby"
                        className="w-48"
                        value={allowance.label}
                        aria-invalid={at('label') || undefined}
                        aria-describedby={at('label') ? labelErrorId : undefined}
                        onChange={(event) => editAllowance(index, 'label', event.target.value)}
                      />
                    </div>
                    <div className="flex flex-col gap-1.5">
                      <Label htmlFor={amountId}>Amount</Label>
                      <Input
                        id={amountId}
                        type="text"
                        inputMode="decimal"
                        placeholder="0.00"
                        className="w-32"
                        value={allowance.amount}
                        aria-invalid={at('amount') || undefined}
                        aria-describedby={at('amount') ? amountErrorId : undefined}
                        onChange={(event) => editAllowance(index, 'amount', event.target.value)}
                      />
                    </div>
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      onClick={() => removeAllowance(index)}
                    >
                      <X className="size-4" aria-hidden="true" />
                      Remove
                    </Button>
                  </div>
                  {lineError?.kind === 'allowance' && lineError.index === index && (
                    <ValidationError
                      id={lineError.field === 'label' ? labelErrorId : amountErrorId}
                    >
                      {lineError.message}
                    </ValidationError>
                  )}
                </li>
              );
            })}
          </ul>
        )}

        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={addAllowance}
          className="self-start"
        >
          <Plus className="size-4" aria-hidden="true" />
          Add a taxable allowance
        </Button>
      </section>

      <section aria-label="Overtime" className="flex flex-col gap-3">
        {overtimes.length === 0 ? (
          <p className="text-sm text-muted-foreground">No overtime on this line.</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {overtimes.map((overtime, index) => {
              const hoursId = `${employmentId}-overtime-hours-${index}`;
              const multiplierId = `${employmentId}-overtime-multiplier-${index}`;
              const labelId = `${employmentId}-overtime-label-${index}`;
              const at = (field: string) =>
                lineError?.kind === 'overtime' &&
                lineError.index === index &&
                lineError.field === field;
              const fieldErrorId = `${errorId}-overtime-${index}`;
              return (
                <li key={index} className="flex flex-col gap-1">
                  <div className="flex flex-col gap-2 sm:flex-row sm:items-end">
                    <div className="flex flex-col gap-1.5">
                      <Label htmlFor={labelId}>Overtime label (optional)</Label>
                      <Input
                        id={labelId}
                        type="text"
                        placeholder="e.g. Sunday overtime"
                        className="w-48"
                        value={overtime.label}
                        aria-invalid={at('label') || undefined}
                        aria-describedby={at('label') ? fieldErrorId : undefined}
                        onChange={(event) => editOvertime(index, 'label', event.target.value)}
                      />
                    </div>
                    <div className="flex flex-col gap-1.5">
                      <Label htmlFor={hoursId}>Hours</Label>
                      <Input
                        id={hoursId}
                        type="text"
                        inputMode="decimal"
                        placeholder="0"
                        className="w-24"
                        value={overtime.hours}
                        aria-invalid={at('hours') || undefined}
                        aria-describedby={at('hours') ? fieldErrorId : undefined}
                        onChange={(event) => editOvertime(index, 'hours', event.target.value)}
                      />
                    </div>
                    <div className="flex flex-col gap-1.5">
                      <Label htmlFor={multiplierId}>Multiplier</Label>
                      <Select
                        id={multiplierId}
                        className="w-28"
                        value={overtime.multiplier}
                        aria-invalid={at('multiplier') || undefined}
                        aria-describedby={at('multiplier') ? fieldErrorId : undefined}
                        onChange={(event) => editOvertime(index, 'multiplier', event.target.value)}
                      >
                        {/* A multiplier stored before this list was what it
                            is today stays visible as itself rather than
                            being silently changed to one of the two. */}
                        {MULTIPLIERS.some((known) => known.value === overtime.multiplier) ? null : (
                          <option value={overtime.multiplier}>{overtime.multiplier} x</option>
                        )}
                        {MULTIPLIERS.map((supported) => (
                          <option key={supported.value} value={supported.value}>
                            {supported.label}
                          </option>
                        ))}
                      </Select>
                    </div>
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      onClick={() => removeOvertime(index)}
                    >
                      <X className="size-4" aria-hidden="true" />
                      Remove
                    </Button>
                  </div>
                  {lineError?.kind === 'overtime' && lineError.index === index && (
                    <ValidationError id={fieldErrorId}>{lineError.message}</ValidationError>
                  )}
                </li>
              );
            })}
          </ul>
        )}

        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={addOvertime}
          className="self-start"
        >
          <Plus className="size-4" aria-hidden="true" />
          Add overtime
        </Button>
        <p className="text-sm text-muted-foreground">
          Type the hours worked. Salt works out the money from this employment&rsquo;s pay and
          recorded ordinary hours, and the workings on the finalized payslip show exactly how.
        </p>
      </section>

      <section aria-label="Medical aid premium" className="flex flex-col gap-3">
        {medicalAidPremiums.length === 0 ? (
          <p className="text-sm text-muted-foreground">No medical aid premium.</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {medicalAidPremiums.map((premium, index) => {
              const amountId = `${employmentId}-medical-aid-amount-${index}`;
              const at =
                lineError?.kind === 'medicalAidPremium' &&
                lineError.index === index &&
                lineError.field === 'amount';
              const amountErrorId = `${errorId}-medical-aid-${index}-amount`;
              return (
                <li key={index} className="flex flex-col gap-1">
                  <div className="flex flex-col gap-2 sm:flex-row sm:items-end">
                    <div className="flex flex-col gap-1.5">
                      <Label htmlFor={amountId}>Premium amount</Label>
                      <Input
                        id={amountId}
                        type="text"
                        inputMode="decimal"
                        placeholder="0.00"
                        className="w-32"
                        value={premium.amount}
                        aria-invalid={at || undefined}
                        aria-describedby={at ? amountErrorId : undefined}
                        onChange={(event) => editMedicalAidPremium(index, event.target.value)}
                      />
                    </div>
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      onClick={() => removeMedicalAidPremium(index)}
                    >
                      <X className="size-4" aria-hidden="true" />
                      Remove
                    </Button>
                  </div>
                  {at && <ValidationError id={amountErrorId}>{lineError?.message}</ValidationError>}
                </li>
              );
            })}
          </ul>
        )}

        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={addMedicalAidPremium}
          className="self-start"
        >
          <Plus className="size-4" aria-hidden="true" />
          Add a medical aid premium
        </Button>
        <p className="text-sm text-muted-foreground">
          Withheld from this employee&rsquo;s own pay, after PAYE and social security, at exactly
          the amount entered. Salt grants no tax relief for it (needs NamRA confirmation).
        </p>
      </section>

      {error !== null && <ValidationError>{error}</ValidationError>}

      <Button type="submit" size="sm" disabled={setEarnings.isPending} className="self-start">
        {setEarnings.isPending ? 'Saving…' : 'Save earnings'}
      </Button>
      <p role="status" className="text-sm text-success">
        {saved ? 'Earnings saved.' : ''}
      </p>
    </form>
  );
}

/** Whether a line about to be sent is the same one already stored, so saving
 * unchanged lines does not raise a false stale-figures warning. Compared
 * field by field per kind: two lines of different kinds are never the same
 * line, whatever fields they happen to share. */
function sameLine(line: EarningLineDto, stored: EarningLineDto | undefined): boolean {
  if (stored === undefined || line.kind !== stored.kind) {
    return false;
  }
  if (line.kind === 'taxableAllowance' && stored.kind === 'taxableAllowance') {
    return line.amountCents === stored.amountCents && line.label === stored.label;
  }
  if (line.kind === 'overtime' && stored.kind === 'overtime') {
    return (
      line.hours === stored.hours &&
      line.multiplier === stored.multiplier &&
      line.label === stored.label
    );
  }
  return false;
}

/** `sameLine`'s counterpart for deduction lines. Exactly one kind exists
 * today, so this compares on amount alone; a second kind would need the
 * same per-kind match `sameLine` uses. */
function sameDeductionLine(line: DeductionLineDto, stored: DeductionLineDto | undefined): boolean {
  return (
    stored !== undefined && line.kind === stored.kind && line.amountCents === stored.amountCents
  );
}
