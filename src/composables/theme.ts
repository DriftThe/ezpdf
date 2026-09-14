import { watch } from "vue";
import { useSettingsStore, type ThemeMode } from "../stores/settings";

/**
 * 主题应用（用户 2026-09-14）：浅色（蓝白）/ 深色 / 跟随系统。
 * - 真相源 = settings.general.theme（config.json 持久化）；localStorage 只存一份
 *   镜像给 index.html 的内联脚本抢在首帧前应用（避免深色用户闪白）。
 * - CSS 全部走 :root[data-theme="dark"] 变量覆盖（main.css），组件不写媒体查询。
 * - 切换时给根节点短暂挂 .theme-switching（颜色过渡 0.3s），避免全局常驻过渡拖慢 hover。
 */

const media = typeof window !== "undefined" ? window.matchMedia("(prefers-color-scheme: dark)") : null;

type Resolved = "light" | "dark";
let applied: Resolved | null = null;
let animTimer = 0;

function resolve(mode: ThemeMode): Resolved {
  if (mode === "dark") return "dark";
  if (mode === "light") return "light";
  return media?.matches ? "dark" : "light";
}

/** 应用主题；animate = 播放一次颜色过渡（用户手动切换时为 true） */
export function applyTheme(mode: ThemeMode, animate = false): void {
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
  root.style.colorScheme = resolved; // 原生控件（select 弹层、滚动条）跟随
}

/** 启动初始化：立即应用持久化值 + 监听切换（动画）+ 系统偏好变化（跟随系统模式） */
export function initTheme(): void {
  const settings = useSettingsStore();
  applyTheme(settings.general.theme);
  watch(
    () => settings.general.theme,
    (mode) => {
      try {
        localStorage.setItem("ezpdf-theme", mode);
      } catch {
        /* 隐私模式等：忽略（不影响功能） */
      }
      applyTheme(mode, true);
    },
  );
  media?.addEventListener("change", () => {
    if (useSettingsStore().general.theme === "system") applyTheme("system", true);
  });
}
