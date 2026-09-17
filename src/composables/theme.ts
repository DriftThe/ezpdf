import { watch } from "vue";
import { useSettingsStore, type ThemeMode } from "../stores/settings";

/** Theme application: system/light/dark, source of truth = general.theme.
 *  localStorage only mirrors it for index.html's pre-first-paint script.
 *  All theming goes through :root[data-theme] vars (main.css); no media queries in components.
 *  .theme-switching adds a 0.3 s color transition only during a switch. */

const media = typeof window !== "undefined" ? window.matchMedia("(prefers-color-scheme: dark)") : null;

type Resolved = "light" | "dark";
let applied: Resolved | null = null;
let animTimer = 0;

function resolve(mode: ThemeMode): Resolved {
  if (mode === "dark") return "dark";
  if (mode === "light") return "light";
  return media?.matches ? "dark" : "light";
}

/** Apply the theme; animate plays a one-shot color transition (true on manual switch). */
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
  root.style.colorScheme = resolved; // native controls (select popups, scrollbars) follow
}

/** Init: apply the persisted value, watch switches (animated) and system preference changes. */
export function initTheme(): void {
  const settings = useSettingsStore();
  applyTheme(settings.general.theme);
  watch(
    () => settings.general.theme,
    (mode) => {
      try {
        localStorage.setItem("ezpdf-theme", mode);
      } catch {
        /* private mode etc.: ignore, no functional impact */
      }
      applyTheme(mode, true);
    },
  );
  media?.addEventListener("change", () => {
    if (useSettingsStore().general.theme === "system") applyTheme("system", true);
  });
}
