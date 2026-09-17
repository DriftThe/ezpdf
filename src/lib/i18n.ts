import { createI18n } from "vue-i18n";
import { LOCALES, messages, type AppLocale } from "../locales";

/** localStorage mirror of the locale, read before mount to avoid a first-frame flash. */
const LANG_MIRROR_KEY = "ezpdf.lang";

export function detectLocale(): AppLocale {
  const tags =
    typeof navigator !== "undefined"
      ? [navigator.language, ...(navigator.languages ?? [])]
      : [];
  for (const raw of tags) {
    const tag = (raw || "").toLowerCase();
    if (!tag) continue;
    if (tag.startsWith("zh")) return /(hant|tw|hk|mo)/.test(tag) ? "zh-TW" : "zh-CN";
    if (tag.startsWith("en")) return "en";
  }
  return "en";
}

export function defaultTargetLang(locale: AppLocale): string {
  if (locale === "zh-CN") return "Simplified Chinese";
  if (locale === "zh-TW") return "Traditional Chinese";
  return "English";
}

export function isAppLocale(v: unknown): v is AppLocale {
  return typeof v === "string" && (LOCALES as string[]).includes(v);
}

function initialLocale(): AppLocale {
  try {
    const saved = localStorage.getItem(LANG_MIRROR_KEY);
    if (isAppLocale(saved)) return saved;
  } catch {
  }
  return detectLocale();
}

export const i18n = createI18n({
  legacy: false,
  globalInjection: true,
  locale: initialLocale(),
  fallbackLocale: "en",
  messages,
});

/** Switch locale and write the localStorage mirror. */
export function setLocale(locale: AppLocale): void {
  i18n.global.locale.value = locale;
  if (typeof document !== "undefined") document.documentElement.lang = locale;
  try {
    localStorage.setItem(LANG_MIRROR_KEY, locale);
  } catch {
  }
}

export function currentLocale(): AppLocale {
  return i18n.global.locale.value as AppLocale;
}

/** For non-component code (stores, composables), where useI18n() is out of reach. */
export function t(key: string, params?: Record<string, unknown>): string {
  return params ? i18n.global.t(key, params) : i18n.global.t(key);
}
