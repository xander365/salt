// `payroll::TaxYear` (crates/payroll/src/tax_year.rs): the Namibian tax year
// runs 1 March to the last day of February, named by the calendar year it
// starts in. Mirrored here only to suggest a sensible default in a form —
// the server is the only thing that ever decides whether a tax year is
// valid for a given date.

export function currentTaxYearStartingYear(today: Date = new Date()): number {
  return today.getMonth() + 1 >= 3 ? today.getFullYear() : today.getFullYear() - 1;
}

/**
 * Parses a tax year typed into a `<input type="number">` into the exact
 * integer the API's `taxYear` field is. `null` for anything that is not a
 * four-digit whole year — `Number()` alone would turn `"2026.5"` into a
 * fraction and an emptied field into `0`, and both reach the server as a
 * `malformed_request` an Operator cannot act on.
 *
 * Shape and required-ness only, never a payroll rule: whether a tax year is
 * one this Employment can be paid in is the server's decision alone.
 */
export function parseTaxYearInput(raw: string): number | null {
  return /^\d{4}$/.test(raw.trim()) ? Number(raw.trim()) : null;
}
