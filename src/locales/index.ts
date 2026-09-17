import common from "./parts/common";
import pipeline from "./parts/pipeline";
import reader from "./parts/reader";
import settings from "./parts/settings";
import shell from "./parts/shell";

/** UI locale; first launch derives it from the system language. */
export type AppLocale = "zh-CN" | "zh-TW" | "en";

export const LOCALES: AppLocale[] = ["zh-CN", "zh-TW", "en"];

/** Each part is keyed off zh-CN (defineMessages guarantees the other two are complete). */
const parts = [common, shell, reader, settings, pipeline];

function merge(locale: AppLocale): Record<string, string> {
  return Object.assign({}, ...parts.map((p) => p[locale])) as Record<string, string>;
}

export const messages: Record<AppLocale, Record<string, string>> = {
  "zh-CN": merge("zh-CN"),
  "zh-TW": merge("zh-TW"),
  en: merge("en"),
};
