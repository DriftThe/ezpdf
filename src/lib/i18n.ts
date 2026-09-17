import { createI18n } from "vue-i18n";
import { LOCALES, messages, type AppLocale } from "../locales";

/** localStorage mirror of the locale, read before mount to avoid a first-frame flash. */
const LANG_MIRROR_KEY = "ezpdf.lang";

/** Map the system language to one of three: hant/tw/hk/mo → zh-TW, other zh → zh-CN,
 *  en → en, anything else falls back to en. */
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

/** Default target language from the system locale (the prompt value Rust interpolates). */
export function defaultTargetLang(locale: AppLocale): string {
  if (locale === "zh-CN") return "Simplified Chinese";
  if (locale === "zh-TW") return "Traditional Chinese";
  return "English";
}

export function isAppLocale(v: unknown): v is AppLocale {
  return typeof v === "string" && (LOCALES as string[]).includes(v);
}

/** Pre-mount initial value: localStorage mirror first, else the system language. */
function initialLocale(): AppLocale {
  try {
    const saved = localStorage.getItem(LANG_MIRROR_KEY);
    if (isAppLocale(saved)) return saved;
  } catch {
    /* localStorage unavailable (private mode): ignore */
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

/** Switch locale and write the mirror (shared by settings + startup restore). */
export function setLocale(locale: AppLocale): void {
  i18n.global.locale.value = locale;
  if (typeof document !== "undefined") document.documentElement.lang = locale;
  try {
    localStorage.setItem(LANG_MIRROR_KEY, locale);
  } catch {
    /* same as above: next launch falls back to detection */
  }
}

/** Current UI locale. */
export function currentLocale(): AppLocale {
  return i18n.global.locale.value as AppLocale;
}

/** Translation entry for non-component code (Pinia stores, composables); useI18n() is out of reach there. */
export function t(key: string, params?: Record<string, unknown>): string {
  return params ? i18n.global.t(key, params) : i18n.global.t(key);
}
