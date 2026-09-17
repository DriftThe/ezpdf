<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { useSettingsStore, type ThemeMode } from "../../stores/settings";

/** Theme toggle: click cycles light → dark → system; persisted in settings.general.theme. */
const { t } = useI18n();
const settings = useSettingsStore();

const ORDER: ThemeMode[] = ["light", "dark", "system"];
const LABEL_KEY: Record<ThemeMode, string> = {
  light: "shell.themeLight",
  dark: "shell.themeDark",
  system: "shell.themeSystem",
};

const mode = computed(() => settings.general.theme);
const title = computed(() => t("shell.themeTitle", { mode: t(LABEL_KEY[mode.value]) }));

function cycle(): void {
  const next = ORDER[(ORDER.indexOf(mode.value) + 1) % ORDER.length];
  void settings.setTheme(next);
}
</script>

<template>
  <button class="icon-btn theme-btn" :title="title" @click="cycle">
    <svg v-if="mode === 'light'" viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round">
      <circle cx="8" cy="8" r="2.6" />
      <path d="M8 1.6v1.6M8 12.8v1.6M1.6 8h1.6M12.8 8h1.6M3.5 3.5l1.1 1.1M11.4 11.4l1.1 1.1M12.5 3.5l-1.1 1.1M4.6 11.4l-1.1 1.1" />
    </svg>
    <svg v-else-if="mode === 'dark'" viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linejoin="round">
      <path d="M13.2 9.6A5.6 5.6 0 1 1 6.4 2.8a4.6 4.6 0 0 0 6.8 6.8z" />
    </svg>
    <svg v-else viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round">
      <rect x="1.8" y="2.6" width="12.4" height="8.4" rx="1.2" />
      <path d="M8 11v2.4M5.4 13.4h5.2" />
    </svg>
  </button>
</template>

<style scoped>
.theme-btn {
  color: var(--text-2);
}
.theme-btn:hover {
  color: var(--text-1);
}
</style>
