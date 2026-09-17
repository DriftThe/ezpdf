<script setup lang="ts">
import { computed, type Component } from "vue";
import { useI18n } from "vue-i18n";
import { useSettingsStore, type SettingsSection } from "../../stores/settings";
import LlmSection from "./sections/LlmSection.vue";
import OcrSection from "./sections/OcrSection.vue";
import CommonSection from "./sections/CommonSection.vue";

const settings = useSettingsStore();
const { t } = useI18n();

/**
 * Section registry (adding a section takes 3 steps; see the SettingsSection comment in stores/settings.ts):
 * the Record keys must cover the SettingsSection union — extending the union without registering here is a compile error.
 */
type SectionDef = { label: string; comp: Component };
const SECTIONS: Record<SettingsSection, SectionDef> = {
  llm: { label: "settings.section.llm", comp: LlmSection },
  ocr: { label: "settings.section.ocr", comp: OcrSection },
  common: { label: "settings.section.common", comp: CommonSection },
};
/** Object.entries keys are string; narrow back to SettingsSection for the nav binding */
const navList = Object.entries(SECTIONS) as Array<[SettingsSection, SectionDef]>;

const activeComp = computed(() => SECTIONS[settings.section].comp);
</script>

<template>
  <!--
    Full-page settings: absolutely positioned over the sidebar + reader (app-shell is the positioning base);
    the toolbar has a higher z-index and stays visible/clickable. The component persists via v-show, so the
    window underneath never unmounts. Switching sections renders only the current section component
    (form state lives in the store, so switching loses nothing).
  -->
  <section v-show="settings.pageOpen" class="settings-page">
    <!-- Left column: back + section nav (reuses the sidebar item look) -->
    <aside class="sp-nav">
      <!-- Back saves: the back button triggers save directly -->
      <button class="btn ghost back-btn" :title="t('settings.backTitle')" @click="settings.save">
        <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round">
          <path d="M10 3L5 8l5 5" />
        </svg>
        {{ t("settings.back") }}
      </button>

      <nav class="sp-items">
        <button
          v-for="[id, def] in navList"
          :key="id"
          class="sp-item"
          :class="{ active: settings.section === id }"
          @click="settings.section = id"
        >
          {{ t(def.label) }}
        </button>
      </nav>
    </aside>

    <!-- Right content area (clears the top toolbar height), renders the current section dynamically -->
    <div class="sp-content">
      <div class="sp-scroll">
        <component :is="activeComp" />
      </div>
    </div>
  </section>
</template>

<style scoped>
.settings-page {
  position: absolute;
  inset: 0;
  z-index: 10; /* toolbar (z-index:20) stays above it, visible and clickable */
  display: flex;
  background: var(--bg-app);
}

/* ---- Left nav (matches the sidebar: same width, background and item spec) ---- */
.sp-nav {
  width: var(--sidebar-w);
  flex: none;
  height: 100%;
  display: flex;
  flex-direction: column;
  background: var(--bg-panel);
  border-right: 1px solid var(--border);
}
.back-btn {
  align-self: flex-start;
  margin: 8px 8px 0;
  color: var(--text-2);
  display: inline-flex;
  align-items: center;
  gap: 6px;
}
.back-btn:hover {
  color: var(--text-1);
}
.sp-items {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 10px 8px;
}
/* Item spec matches the enlarged repo tree (34px row / 17px font) */
.sp-item {
  border: none;
  background: transparent;
  height: 34px;
  padding: 0 10px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  color: var(--text-2);
  font-size: 17px;
  text-align: left;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.sp-item:hover {
  background: var(--bg-hover);
  color: var(--text-1);
}
.sp-item.active {
  background: var(--accent-weak);
  color: var(--accent);
  font-weight: 600;
}

/* ---- Right content ---- */
.sp-content {
  flex: 1;
  min-width: 0;
  height: 100%;
  display: flex;
  flex-direction: column;
  padding-top: var(--toolbar-h); /* clear the persistent top toolbar */
}
.sp-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
}
</style>
