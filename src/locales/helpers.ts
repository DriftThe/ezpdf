/** zh-CN's key set is authoritative; a missing/extra key in zh-TW/en is a type error. */
export function defineMessages<T extends Record<string, string>>(m: {
  "zh-CN": T;
  "zh-TW": Record<keyof T & string, string>;
  en: Record<keyof T & string, string>;
}): { "zh-CN": T; "zh-TW": Record<keyof T & string, string>; en: Record<keyof T & string, string> } {
  return m;
}
