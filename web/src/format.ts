// Human-readable date display (issue #88, DESIGN.md's copy conventions).
//
// Only for *displaying* a date already recorded — a `type="date"` input
// always keeps the browser's own ISO value, and a confirmation sentence that
// echoes what an Operator just typed keeps that exact string too. This is
// for the third case: a date shown back as data (a list, a header, a
// record), where "7 Sep 2026" reads faster and cannot be misread as
// day-first or month-first the way "07/09/26" can.

const MONTHS = [
  'Jan',
  'Feb',
  'Mar',
  'Apr',
  'May',
  'Jun',
  'Jul',
  'Aug',
  'Sep',
  'Oct',
  'Nov',
  'Dec',
] as const;

/** `"2026-09-07"` → `"7 Sep 2026"`. Returns the input unchanged if it is not
 * a plain `YYYY-MM-DD` string — an honest fallback beats a display that
 * throws over one unexpected value. */
export function humanDate(iso: string): string {
  const match = /^(\d{4})-(\d{2})-(\d{2})$/.exec(iso);
  if (match === null) {
    return iso;
  }

  const [, year, month, day] = match;
  const monthIndex = Number(month) - 1;
  if (monthIndex < 0 || monthIndex > 11) {
    return iso;
  }

  return `${Number(day)} ${MONTHS[monthIndex]} ${year}`;
}

/** `humanDate` for a period, joined the same way every screen already joins
 * one. */
export function humanDateRange(start: string, end: string): string {
  return `${humanDate(start)} – ${humanDate(end)}`;
}
