/**
 * 目标语言预设（用户 2026-09-15）：下拉里给常见目标语言，避免手输；仍保留"自定义"
 * 以覆盖预设之外的说法（如 Cantonese / English (US)）。
 *
 * value = 直接写进提示词的语言名（英文，模型识别最稳）；label = 母语名 + 英文名，
 * 各界面语言下都能认（故不进 i18n 目录）。
 */
export interface TargetLangOption {
  value: string;
  label: string;
}

/** 选中即用输入框的自由写法（不写死语言名） */
export const CUSTOM_TARGET_LANG = "__custom__";

export const TARGET_LANG_OPTIONS: readonly TargetLangOption[] = [
  { value: "Simplified Chinese", label: "简体中文 (Simplified Chinese)" },
  { value: "Traditional Chinese", label: "繁體中文 (Traditional Chinese)" },
  { value: "English", label: "English" },
  { value: "Japanese", label: "日本語 (Japanese)" },
  { value: "Korean", label: "한국어 (Korean)" },
  { value: "French", label: "Français (French)" },
  { value: "German", label: "Deutsch (German)" },
  { value: "Spanish", label: "Español (Spanish)" },
  { value: "Portuguese", label: "Português (Portuguese)" },
  { value: "Italian", label: "Italiano (Italian)" },
  { value: "Russian", label: "Русский (Russian)" },
  { value: "Ukrainian", label: "Українська (Ukrainian)" },
  { value: "Polish", label: "Polski (Polish)" },
  { value: "Dutch", label: "Nederlands (Dutch)" },
  { value: "Czech", label: "Čeština (Czech)" },
  { value: "Romanian", label: "Română (Romanian)" },
  { value: "Turkish", label: "Türkçe (Turkish)" },
  { value: "Arabic", label: "العربية (Arabic)" },
  { value: "Persian", label: "فارسی (Persian)" },
  { value: "Hebrew", label: "עברית (Hebrew)" },
  { value: "Hindi", label: "हिन्दी (Hindi)" },
  { value: "Vietnamese", label: "Tiếng Việt (Vietnamese)" },
  { value: "Thai", label: "ไทย (Thai)" },
  { value: "Indonesian", label: "Bahasa Indonesia (Indonesian)" },
  { value: "Malay", label: "Bahasa Melayu (Malay)" },
];

/** 当前值是否命中预设（否则下拉显示"自定义"并展开输入框） */
export function isPresetTargetLang(value: string): boolean {
  return TARGET_LANG_OPTIONS.some((o) => o.value === value);
}
