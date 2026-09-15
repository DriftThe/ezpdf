<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { useSettingsStore } from "../../../stores/settings";
import type { AppLocale } from "../../../locales";

const settings = useSettingsStore();
const { t } = useI18n();

function onLang(e: Event): void {
  void settings.setLang((e.target as HTMLSelectElement).value as AppLocale);
}
</script>

<template>
  <div class="set-pane">
    <h2 class="set-title">{{ t("settings.common.title") }}</h2>
    <div class="set-field">
      <span>{{ t("settings.general.uiLang") }}</span>
      <span class="set-select-wrap">
        <select class="set-select" :value="settings.general.lang" @change="onLang">
          <option value="zh-CN">{{ t("settings.general.langZhCN") }}</option>
          <option value="zh-TW">{{ t("settings.general.langZhTW") }}</option>
          <option value="en">{{ t("settings.general.langEn") }}</option>
        </select>
        <svg class="set-select-arrow" viewBox="0 0 10 6" width="10" height="6" aria-hidden="true">
          <path d="M1 1l4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
        </svg>
      </span>
    </div>
    <label class="set-check">
      <input v-model="settings.general.autoLaunch" type="checkbox" />
      <span>{{ t("settings.general.autoLaunch") }}</span>
    </label>
    <p class="set-hint">
      {{ t("settings.general.autoLaunchHint") }}
    </p>
    <label class="set-check">
      <input v-model="settings.general.resumeOnStart" type="checkbox" />
      <span>{{ t("settings.general.resumeOnStart") }}</span>
    </label>
    <p class="set-hint">
      {{ t("settings.general.resumeOnStartHint") }}
    </p>
    <p class="set-hint">{{ t("settings.general.saveHint") }}</p>
    <!-- 更新检查（用户 2026-09-14）：启动静默一次，这里手动重查 -->
    <div class="set-field">
      <span>{{ t("update.title") }}</span>
      <div class="set-field-row update-row">
        <span class="set-hint">v{{ settings.appVersion || "…" }} · {{ settings.updateText }}</span>
        <button class="set-button" :disabled="settings.updateBusy" @click="settings.checkUpdate(true)">
          {{ settings.updateBusy ? t("update.checking") : t("update.check") }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.update-row {
  gap: 10px;
  align-items: center;
}
</style>
