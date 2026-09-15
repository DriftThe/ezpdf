<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { useSettingsStore } from "../../../stores/settings";
import type { AppLocale } from "../../../locales";
import { BLOCK_TYPE_OPTIONS, DEFAULT_TRANSLATED_TYPES } from "../../../lib/blocks";

const settings = useSettingsStore();
const { t } = useI18n();

function onLang(e: Event): void {
  void settings.setLang((e.target as HTMLSelectElement).value as AppLocale);
}

/** 块类型勾选：直接改 general.translateTypes（随设置页返回键整份保存） */
function onToggleType(kind: string, e: Event): void {
  const on = (e.target as HTMLInputElement).checked;
  const cur = new Set(settings.general.translateTypes);
  if (on) cur.add(kind);
  else cur.delete(kind);
  // 保持稳定顺序（按 BLOCK_TYPE_OPTIONS 排列），便于配置对比与日志阅读
  settings.general.translateTypes = BLOCK_TYPE_OPTIONS.filter((k) => cur.has(k));
}

function resetTypes(): void {
  settings.general.translateTypes = [...DEFAULT_TRANSLATED_TYPES];
}

function isTypeOn(kind: string): boolean {
  return settings.general.translateTypes.includes(kind);
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

    <!-- 翻译块类型（用户 2026-09-15）：勾选的类型送翻并在译文栏覆盖显示 -->
    <h3 class="block-type-title">{{ t("settings.general.blockTypes") }}</h3>
    <p class="set-hint">{{ t("settings.general.blockTypesHint") }}</p>
    <div class="block-types">
      <label v-for="kind in BLOCK_TYPE_OPTIONS" :key="kind" class="set-check block-type">
        <input type="checkbox" :checked="isTypeOn(kind)" @change="onToggleType(kind, $event)" />
        <span>{{ t(`settings.blockType.${kind}`) }}</span>
      </label>
    </div>
    <button class="set-button" @click="resetTypes">{{ t("settings.general.blockTypesReset") }}</button>
    <!-- 更新检查（用户 2026-09-14）：启动静默一次，这里手动重查 -->
    <div class="set-field">
      <span>{{ t("update.title") }}</span>
      <div class="set-field-row update-row">
        <span class="update-text">v{{ settings.appVersion || "…" }} · {{ settings.updateText }}</span>
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
/* 版本行文本：不复用 .set-hint（它的负上边距会把整行往下顶，和左侧标签对不齐） */
.update-text {
  margin: 0;
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-3);
}
/* 翻译块类型：小标题 + 多列勾选网格 */
.block-type-title {
  margin: 18px 0 6px;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-1);
}
.block-types {
  display: flex;
  flex-wrap: wrap;
  gap: 2px 28px;
  max-width: 560px;
}
.block-type {
  margin: 4px 0;
  min-width: 132px;
}
</style>
