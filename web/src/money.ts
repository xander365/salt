// Money crosses this application only as a whole number of cents (INV-001:
// never `f32`/`f64`, issue #63's own acceptance criterion). Both directions
// below stay in string and integer arithmetic on purpose — `parseFloat` or
// `(cents / 100).toFixed(2)` would let an amount pick up floating-point
// error on its way onto or off the wire.

/**
 * Parses an amount typed as whole currency units (e.g. "1500" or "1500.50")
 * into an exact integer number of cents. `null` for anything that is not a
 * non-negative amount with at most two decimal places — Salt's own `Money`
 * is never negative and never fractional at cents precision.
 */
export function parseCentsInput(raw: string): number | null {
  const match = /^(\d+)(?:\.(\d{1,2}))?$/.exec(raw.trim());
  if (match === null) {
    return null;
  }
  const [, whole, fraction = ''] = match;
  return Number(whole) * 100 + Number(fraction.padEnd(2, '0'));
}

/** The inverse of {@link parseCentsInput}, for displaying a recorded amount. */
export function formatCents(cents: number): string {
  const whole = Math.trunc(cents / 100);
  const remainder = String(cents % 100).padStart(2, '0');
  return `${whole}.${remainder}`;
}
