/**
 * 二分出「填满且不溢出」的最大字号（上下界内）。
 *
 * 两处覆盖框共用（PageCard 文本块、TableCover 表格）：`fits(size)` 由调用方给
 * ——文本块只看 scrollHeight，表格还要看 scrollWidth，判据不同但搜索逻辑一样。
 * 返回 min 有两种含义，调用方按各自策略兜底：最小号就放得下 → 真实最大号；
 * 最小号仍溢出 → 保持最小号（文本块靠 overflow 截断，表格整框透明回退原像素）。
 */
export function fitFontSize(fits: (size: number) => boolean, min: number, max: number): number {
  if (!fits(min)) return min;
  let lo = min;
  let hi = Math.max(min, max);
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (fits(mid)) lo = mid;
    else hi = mid - 1;
  }
  // 收敛到最终值：循环里最后一次探测可能是"失败"的更大字号，
  // 不重设的话元素停留在溢出字号上（部分框溢出的根因）
  return lo;
}
