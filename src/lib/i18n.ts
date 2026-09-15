import { createI18n } from "vue-i18n";
import { LOCALES, messages, type AppLocale } from "../locales";

/** 界面语言在 localStorage 的镜像（与主题同理：挂载前先读，避免首帧语言闪变） */
const LANG_MIRROR_KEY = "ezpdf.lang";

/**
 * 系统语言 → 支持的三选一：
 * 繁体（zh-Hant / TW / HK / MO）→ zh-TW，其余中文 → zh-CN，英文 → en，
 * 其他语言一律回落 en（产品决定：跟随系统，不做猜译）。
 */
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

/** 首启目标语言（用户 2026-09-15：跟随系统；值为提示词里的语言名，Rust 直接内插） */
export function defaultTargetLang(locale: AppLocale): string {
  if (locale === "zh-CN") return "Simplified Chinese";
  if (locale === "zh-TW") return "Traditional Chinese";
  return "English";
}

export function isAppLocale(v: unknown): v is AppLocale {
  return typeof v === "string" && (LOCALES as string[]).includes(v);
}

/** 挂载前可用的初值：localStorage 镜像优先（上次选过的），否则系统语言 */
function initialLocale(): AppLocale {
  try {
    const saved = localStorage.getItem(LANG_MIRROR_KEY);
    if (isAppLocale(saved)) return saved;
  } catch {
    /* localStorage 不可用（隐私模式）：忽略 */
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

/** 切换界面语言并写镜像（设置页与启动恢复共用） */
export function setLocale(locale: AppLocale): void {
  i18n.global.locale.value = locale;
  if (typeof document !== "undefined") document.documentElement.lang = locale;
  try {
    localStorage.setItem(LANG_MIRROR_KEY, locale);
  } catch {
    /* 同上：写不进就让下次启动回落检测值 */
  }
}

/** 当前界面语言 */
export function currentLocale(): AppLocale {
  return i18n.global.locale.value as AppLocale;
}

/**
 * 非组件上下文（Pinia store / composable / 工具函数）的翻译入口：
 * setup 外的代码拿不到 useI18n()，统一走 global 实例。
 */
export function t(key: string, params?: Record<string, unknown>): string {
  return params ? i18n.global.t(key, params) : i18n.global.t(key);
}
