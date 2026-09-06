// Money crosses this application only as a whole number of cents (INV-001:
// never `f32`/`f64`, issue #63's own acceptance criterion). Both directions
// below stay in string and integer arithmetic on purpose — `parseFloat` or
// `(cents / 100).toFixed(2)` would let an amount pick up floating-point
// error on its way onto or off the wire.

/**
 * Parses an amount typed as whole currency units (e.g. "1500" or "1500.50")
 * into an exact integer number of cents. `null` for anything that is not a
 * non-negative amount with at most two decimal places whose cents fit in
 * JavaScript's safe-integer range — Salt's own `Money` is never negative or
 * fractional, and the browser refuses rather than rounds a larger value.
 */
export function parseCentsInput(raw: string): number | null {
  const match = /^(\d+)(?:\.(\d{1,2}))?$/.exec(raw.trim());
  if (match === null) {
    return null;
  }

  const [, whole, fraction = ''] = match;
  const exactCents = BigInt(whole) * 100n + BigInt(fraction.padEnd(2, '0'));
  if (exactCents > BigInt(Number.MAX_SAFE_INTEGER)) {
    return null;
  }
  return Number(exactCents);
}

/** The inverse of {@link parseCentsInput}, for displaying a recorded amount. */
export function formatCents(cents: number): string {
  if (!Number.isSafeInteger(cents) || cents < 0) {
    throw new RangeError('cents must be a non-negative safe integer');
  }

  const exactCents = BigInt(cents);
  const whole = exactCents / 100n;
  const remainder = String(exactCents % 100n).padStart(2, '0');
  return `${whole}.${remainder}`;
}

/**
 * {@link formatCents} for a screen: it throws rather than show an amount it
 * cannot render exactly (INV-001), and thrown from inside a component that
 * would blank the whole screen over one figure, hiding every other one. The
 * one figure says what it cannot show instead, and the rest still reads.
 */
export function centsText(cents: number): string {
  try {
    return formatCents(cents);
  } catch {
    return 'an amount that cannot be displayed exactly';
  }
}
