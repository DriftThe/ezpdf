/** Binary-search the largest fitting size; returning min is ambiguous — if min overflows, keep it. */
export function fitFontSize(fits: (size: number) => boolean, min: number, max: number): number {
  if (!fits(min)) return min;
  let lo = min;
  let hi = Math.max(min, max);
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (fits(mid)) lo = mid;
    else hi = mid - 1;
  }
  // the last probe may have failed; without setting the element stays overflowing
  return lo;
}
