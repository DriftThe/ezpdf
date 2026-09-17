/** Compile-time integrity for the three locales: zh-CN's key set is authoritative;
 *  a missing or extra key in zh-TW/en is a type error here. */
export function defineMessages<T extends Record<string, string>>(m: {
  "zh-CN": T;
  "zh-TW": Record<keyof T & string, string>;
  en: Record<keyof T & string, string>;
}): { "zh-CN": T; "zh-TW": Record<keyof T & string, string>; en: Record<keyof T & string, string> } {
  return m;
}
