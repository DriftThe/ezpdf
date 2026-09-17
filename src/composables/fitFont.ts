/** Binary-search the largest font size that fills without overflowing, within bounds.
 *  Callers supply `fits` (text blocks check scrollHeight, tables also scrollWidth).
 *  Returning min is ambiguous: if min fits it is the real max; if min overflows, keep it
 *  (text blocks clip via overflow, tables go transparent to reveal the original pixels). */
export function fitFontSize(fits: (size: number) => boolean, min: number, max: number): number {
  if (!fits(min)) return min;
  let lo = min;
  let hi = Math.max(min, max);
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (fits(mid)) lo = mid;
    else hi = mid - 1;
  }
  // converge: the last probe may have been a too-large size that failed, so without
  // setting the element stays at an overflowing size
  return lo;
}
