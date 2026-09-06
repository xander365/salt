// `payroll::TaxYear` (crates/payroll/src/tax_year.rs): the Namibian tax year
// runs 1 March to the last day of February, named by the calendar year it
// starts in. Mirrored here only to suggest a sensible default in a form —
// the server is the only thing that ever decides whether a tax year is
// valid for a given date.

export function currentTaxYearStartingYear(today: Date = new Date()): number {
  return today.getMonth() + 1 >= 3 ? today.getFullYear() : today.getFullYear() - 1;
}
