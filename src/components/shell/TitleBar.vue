<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { isTauri } from "../../lib/env";
import ThemeToggle from "./ThemeToggle.vue";

/** Self-drawn title bar (decorations off): drag via `data-tauri-drag-region` (buttons excluded),
 *  double-click maximizes. Inert in the browser. */
const { t } = useI18n();
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
    /* window API unavailable: keep the default icon */
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
    <button class="win-btn" :title="t('shell.minimize')" @click="minimize">
      <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
        <path d="M0.5 5h9" stroke="currentColor" stroke-width="1.1" />
      </svg>
    </button>
    <button class="win-btn" :title="maximized ? t('shell.restore') : t('shell.maximize')" @click="toggleMaximize">
      <svg v-if="!maximized" viewBox="0 0 10 10" width="10" height="10" fill="none" aria-hidden="true">
        <rect x="0.8" y="0.8" width="8.4" height="8.4" stroke="currentColor" stroke-width="1.1" />
      </svg>
      <svg v-else viewBox="0 0 10 10" width="10" height="10" fill="none" aria-hidden="true">
        <rect x="0.8" y="2.6" width="6.6" height="6.6" stroke="currentColor" stroke-width="1.1" />
        <path d="M2.6 2.6V0.8h6.6v6.6H7.4" stroke="currentColor" stroke-width="1.1" />
      </svg>
    </button>
    <button class="win-btn close" :title="t('common.close')" @click="close">
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
.bar-drag {
  flex: 1;
  align-self: stretch;
}
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
