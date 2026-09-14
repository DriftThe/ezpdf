<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { getCurrentWindow } from "@tauri-apps/api/window";
import ThemeToggle from "./ThemeToggle.vue";

/**
 * 自绘标题栏（用户 2026-09-14）：不启用系统装饰（tauri.conf `decorations: false`），
 * 左侧只有 ezpdf 字标，右侧主题切换 + 最小化/最大化(还原)/关闭。
 * 拖动：整条挂 `data-tauri-drag-region`（按钮不挂，权限见 capabilities）；双击切换最大化。
 * 浏览器 dev（无 Tauri）下按钮静默无效。
 */
const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
const win = isTauri ? getCurrentWindow() : null;
const maximized = ref(false);
let unlisten: (() => void) | null = null;

onMounted(async () => {
  if (!win) return;
  try {
    maximized.value = await win.isMaximized();
    unlisten = await win.onResized(() => {
      void win.isMaximized().then((v) => (maximized.value = v));
    });
  } catch {
    /* 窗口 API 不可用：保持默认图标 */
  }
});
onBeforeUnmount(() => unlisten?.());

function minimize(): void {
  void win?.minimize();
}
function toggleMaximize(): void {
  void win?.toggleMaximize();
}
function close(): void {
  void win?.close();
}
</script>

<template>
  <header class="titlebar" data-tauri-drag-region @dblclick="toggleMaximize">
    <span class="app-name" data-tauri-drag-region>ezpdf</span>
    <div class="bar-drag" data-tauri-drag-region />
    <ThemeToggle />
    <button class="win-btn" title="最小化" @click="minimize">
      <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
        <path d="M0.5 5h9" stroke="currentColor" stroke-width="1.1" />
      </svg>
    </button>
    <button class="win-btn" :title="maximized ? '还原' : '最大化'" @click="toggleMaximize">
      <svg v-if="!maximized" viewBox="0 0 10 10" width="10" height="10" fill="none" aria-hidden="true">
        <rect x="0.8" y="0.8" width="8.4" height="8.4" stroke="currentColor" stroke-width="1.1" />
      </svg>
      <svg v-else viewBox="0 0 10 10" width="10" height="10" fill="none" aria-hidden="true">
        <rect x="0.8" y="2.6" width="6.6" height="6.6" stroke="currentColor" stroke-width="1.1" />
        <path d="M2.6 2.6V0.8h6.6v6.6H7.4" stroke="currentColor" stroke-width="1.1" />
      </svg>
    </button>
    <button class="win-btn close" title="关闭" @click="close">
      <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
        <path d="M0.8 0.8l8.4 8.4M9.2 0.8L0.8 9.2" stroke="currentColor" stroke-width="1.1" />
      </svg>
    </button>
  </header>
</template>

<style scoped>
.titlebar {
  height: var(--titlebar-h);
  flex: none;
  display: flex;
  align-items: center;
  gap: 6px;
  padding-left: 12px;
  background: var(--bg-panel);
  border-bottom: 1px solid var(--border);
  user-select: none;
}
.app-name {
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.4px;
  color: var(--text-2);
}
/* 空白拖动区：占满中段 */
.bar-drag {
  flex: 1;
  align-self: stretch;
}
/* Windows 风格窗控：整条高度、无圆角，关闭键悬停红底 */
.win-btn {
  width: 44px;
  height: 100%;
  flex: none;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 0;
  border: none;
  background: transparent;
  color: var(--text-2);
  cursor: default;
}
.win-btn:hover {
  background: var(--bg-hover);
  color: var(--text-1);
}
.win-btn.close:hover {
  background: #e81123;
  color: #fff;
}
</style>
