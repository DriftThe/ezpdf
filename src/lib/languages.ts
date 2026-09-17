/** value = English name written into the prompt; label = native + English (needs no i18n key). */
interface TargetLangOption {
  value: string;
  label: string;
}

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

export function isPresetTargetLang(value: string): boolean {
  return TARGET_LANG_OPTIONS.some((o) => o.value === value);
}
