// The `payslip_particulars_not_frozen` refusal's own `details.missing`
// (parent #70 D-9): the frozen records a Payslip needs that a payroll
// finalized before issue #73 never froze. Shared between one Payslip's own
// download (`PayslipDownload.tsx`, issue #82) and a whole run's batch
// download (`RunPayslipsDownload.tsx`, issue #83) — both name the same
// missing records in the same words.

/** The frozen records `payslip_particulars_not_frozen` names, in the
 * server's own words, read as the sentence an Operator needs. Unknown names
 * are passed through rather than dropped, so the reason is never shortened. */
const MISSING_RECORD_NAMES: Record<string, string> = {
  EmployerParticulars: 'the employer particulars',
  PersonParticulars: 'the employee particulars',
  PayslipTemplateVersion: 'the payslip template version',
};

export function missingRecordsOf(details: unknown): string[] {
  if (typeof details !== 'object' || details === null) {
    return [];
  }
  const missing = (details as { missing?: unknown }).missing;
  return Array.isArray(missing)
    ? missing.filter((name): name is string => typeof name === 'string')
    : [];
}

function joinNames(names: string[]): string {
  if (names.length <= 1) {
    return names.join('');
  }
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`;
}

/** The named missing records as a sentence fragment ("the employer
 * particulars and the payslip template version"), or the honest fallback
 * when `details` carried none this app recognizes. */
export function missingParticularsPhrase(details: unknown): string {
  const missing = missingRecordsOf(details).map((name) => MISSING_RECORD_NAMES[name] ?? name);
  return missing.length > 0 ? joinNames(missing) : 'the particulars a payslip must print';
}
