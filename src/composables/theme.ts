import { watch } from "vue";
import { useSettingsStore, type ThemeMode } from "../stores/settings";

/** general.theme is the source of truth; localStorage only feeds index.html's pre-paint script. */

const media = typeof window !== "undefined" ? window.matchMedia("(prefers-color-scheme: dark)") : null;

type Resolved = "light" | "dark";
let applied: Resolved | null = null;
let animTimer = 0;

function resolve(mode: ThemeMode): Resolved {
  if (mode === "dark") return "dark";
  if (mode === "light") return "light";
  return media?.matches ? "dark" : "light";
}

function applyTheme(mode: ThemeMode, animate = false): void {
  const resolved = resolve(mode);
  if (resolved === applied) return;
  applied = resolved;
  const root = document.documentElement;
  if (animate) {
    root.classList.add("theme-switching");
    window.clearTimeout(animTimer);
    animTimer = window.setTimeout(() => root.classList.remove("theme-switching"), 380);
  }
  root.dataset.theme = resolved;
  root.style.colorScheme = resolved;
}

export function initTheme(): void {
  const settings = useSettingsStore();
  applyTheme(settings.general.theme);
  watch(
    () => settings.general.theme,
    (mode) => {
      try {
        localStorage.setItem("ezpdf-theme", mode);
      } catch {
      }
      applyTheme(mode, true);
    },
  );
  media?.addEventListener("change", () => {
    if (useSettingsStore().general.theme === "system") applyTheme("system", true);
  });
}
