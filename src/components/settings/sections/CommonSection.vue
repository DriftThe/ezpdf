<script setup lang="ts">
import { useI18n } from "vue-i18n";
import { useSettingsStore } from "../../../stores/settings";
import type { AppLocale } from "../../../locales";
import { BLOCK_TYPE_OPTIONS, DEFAULT_TRANSLATED_TYPES } from "../../../lib/blocks";
import SelectArrow from "../../common/SelectArrow.vue";

const settings = useSettingsStore();
const { t } = useI18n();

function onLang(e: Event): void {
  void settings.setLang((e.target as HTMLSelectElement).value as AppLocale);
}

/** Block type checkbox: edits general.translateTypes directly (saved wholesale by the settings back button) */
function onToggleType(kind: string, e: Event): void {
  const on = (e.target as HTMLInputElement).checked;
  const cur = new Set(settings.general.translateTypes);
  if (on) cur.add(kind);
  else cur.delete(kind);
  // Keep a stable order (per BLOCK_TYPE_OPTIONS) for easier config diffing and log reading
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
        <SelectArrow />
      </span>
    </div>
    <label class="set-check">
      <input v-model="settings.general.autoLaunch" type="checkbox" />
      <span>{{ t("settings.general.autoLaunch") }}</span>
    </label>
    <label class="set-check">
      <input v-model="settings.general.resumeOnStart" type="checkbox" />
      <span>{{ t("settings.general.resumeOnStart") }}</span>
    </label>

    <!-- Translation block types: checked types are translated and covered in the translation pane -->
    <h3 class="block-type-title">{{ t("settings.general.blockTypes") }}</h3>
    <div class="block-types">
      <label v-for="kind in BLOCK_TYPE_OPTIONS" :key="kind" class="set-check block-type">
        <input type="checkbox" :checked="isTypeOn(kind)" @change="onToggleType(kind, $event)" />
        <span>{{ t(`settings.blockType.${kind}`) }}</span>
      </label>
    </div>
    <button class="set-button" @click="resetTypes">{{ t("settings.general.blockTypesReset") }}</button>
    <!-- Update check: silent once at launch, re-run manually here -->
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
/* Version line text: doesn't reuse .set-hint (its negative top margin pushes the line down and misaligns it with the left label) */
.update-text {
  margin: 0;
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-3);
}
/* Translation block types: subheading + multi-column checkbox grid */
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
