import common from "./parts/common";
import pipeline from "./parts/pipeline";
import reader from "./parts/reader";
import settings from "./parts/settings";
import shell from "./parts/shell";

/** 界面语言（用户 2026-09-15）：简中 / 繁中 / English；首启按系统语言定初值 */
export type AppLocale = "zh-CN" | "zh-TW" | "en";

export const LOCALES: AppLocale[] = ["zh-CN", "zh-TW", "en"];

/** 各区域目录以 zh-CN 的 key 为基准（helpers.defineMessages 保证另两语不缺 key） */
const parts = [common, shell, reader, settings, pipeline];

function merge(locale: AppLocale): Record<string, string> {
  return Object.assign({}, ...parts.map((p) => p[locale])) as Record<string, string>;
}

export const messages: Record<AppLocale, Record<string, string>> = {
  "zh-CN": merge("zh-CN"),
  "zh-TW": merge("zh-TW"),
  en: merge("en"),
};
