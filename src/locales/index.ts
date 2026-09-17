import common from "./parts/common";
import pipeline from "./parts/pipeline";
import reader from "./parts/reader";
import settings from "./parts/settings";
import shell from "./parts/shell";

export type AppLocale = "zh-CN" | "zh-TW" | "en";

export const LOCALES: AppLocale[] = ["zh-CN", "zh-TW", "en"];

const parts = [common, shell, reader, settings, pipeline];

function merge(locale: AppLocale): Record<string, string> {
  return Object.assign({}, ...parts.map((p) => p[locale])) as Record<string, string>;
}

export const messages: Record<AppLocale, Record<string, string>> = {
  "zh-CN": merge("zh-CN"),
  "zh-TW": merge("zh-TW"),
  en: merge("en"),
};
