/**
 * 三语目录的编译期完整性约束：以 zh-CN 的 key 集合为准，
 * zh-TW / en 少写或多写一个 key 都会在这里报类型错误。
 */
export function defineMessages<T extends Record<string, string>>(m: {
  "zh-CN": T;
  "zh-TW": Record<keyof T & string, string>;
  en: Record<keyof T & string, string>;
}): { "zh-CN": T; "zh-TW": Record<keyof T & string, string>; en: Record<keyof T & string, string> } {
  return m;
}
