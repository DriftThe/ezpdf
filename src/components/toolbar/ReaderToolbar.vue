<script setup lang="ts">
import { computed } from "vue";
import { useI18n } from "vue-i18n";
import { useLibraryStore } from "../../stores/library";
import { useParseStore } from "../../stores/parse";
import { useReaderStore } from "../../stores/reader";
import { useSettingsStore } from "../../stores/settings";
import type { LayoutMode } from "../../types/domain";

const { t } = useI18n();
const lib = useLibraryStore();
const parse = useParseStore();
const reader = useReaderStore();
const settings = useSettingsStore();

const hasPdf = computed(() => !!lib.currentPdf);

const layoutOptions = computed<Array<{ value: LayoutMode; label: string; title: string }>>(() => [
  { value: "ot", label: t("toolbar.layoutOtLabel"), title: t("toolbar.layoutOtTitle") },
  { value: "to", label: t("toolbar.layoutToLabel"), title: t("toolbar.layoutToTitle") },
  { value: "o", label: t("toolbar.layoutOLabel"), title: t("toolbar.layoutOTitle") },
  { value: "t", label: t("toolbar.layoutTLabel"), title: t("toolbar.layoutTTitle") },
]);

const svcText = computed(
  () =>
    ({
      unknown: t("toolbar.svcUnknown"),
      starting: t("toolbar.svcStarting"),
      connected: t("toolbar.svcConnected"),
      disconnected: t("toolbar.svcDisconnected"),
      failed: t("toolbar.svcFailed"),
    })[parse.serviceStatus],
);

function onPageInput(e: Event): void {
  const v = Number((e.target as HTMLInputElement).value);
  if (Number.isFinite(v) && v > 0) reader.gotoPage(v);
}

/** Fit-width button: clickable only in manual zoom mode (no-op when already fit) */
function onFitWidthClick(): void {
  if (reader.fitMode === "custom") reader.toggleFit();
}
</script>

<template>
  <header class="toolbar">
    <button class="icon-btn" :disabled="settings.pageOpen" :title="lib.sidebarOpen ? t('toolbar.collapseSidebar') : t('toolbar.expandSidebar')" @click="lib.toggleSidebar">
      <svg viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round">
        <path d="M2.5 4h11M2.5 8h11M2.5 12h11" />
      </svg>
    </button>

    <span class="divider" />

    <div class="seg" role="group" :aria-label="t('toolbar.layoutAria')">
      <button
        v-for="opt in layoutOptions"
        :key="opt.value"
        class="seg-btn"
        :class="{ active: reader.layout === opt.value }"
        :title="opt.title"
        @click="reader.setLayout(opt.value)"
      >
        {{ opt.label }}
      </button>
    </div>

    <span class="divider" />

    <!-- Zoom percentage is the real ratio; click toggles fit-width / 100% -->
    <div class="zoom">
      <button class="icon-btn" :title="t('toolbar.zoomOut')" @click="reader.zoomOut">−</button>
      <button
        class="zoom-val"
        :title="reader.fitMode === 'width' ? t('toolbar.fitWidthActive') : t('toolbar.clickFitWidth')"
        @click="reader.toggleFit"
      >
        {{ Math.round(reader.effectiveZoom * 100) }}%
      </button>
      <button class="icon-btn" :title="t('toolbar.zoomIn')" @click="reader.zoomIn">＋</button>
      <button class="icon-btn fit-btn" :title="t('toolbar.fitWidth')" :class="{ active: reader.fitMode === 'width' }" @click="onFitWidthClick">
        <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round">
          <path d="M1.5 5V2.5H4M12 2.5h2.5V5M14.5 11v2.5H12M4 13.5H1.5V11" />
          <path d="M4.5 8h7" />
        </svg>
      </button>
    </div>

    <span class="divider" />

    <div class="pagenav">
      <button class="icon-btn" :title="t('toolbar.prevPage')" :disabled="!hasPdf || reader.currentPage <= 1" @click="reader.stepPage(-1)">‹</button>
      <span class="page-ind">
        <input
          class="page-input"
          type="number"
          min="1"
          :max="reader.pageCount || 1"
          :value="reader.currentPage"
          @change="onPageInput"
        />
        <span class="page-total">/ {{ reader.pageCount || "–" }}</span>
      </span>
      <button class="icon-btn" :title="t('toolbar.nextPage')" :disabled="!hasPdf || reader.currentPage >= reader.pageCount" @click="reader.stepPage(1)">›</button>
    </div>

    <span class="divider" />

    <label class="switch" :title="t('toolbar.hoverPreviewTitle')">
      <input v-model="reader.hoverPreview" type="checkbox" />
      <span class="track"><span class="thumb" /></span>
      {{ t("toolbar.hoverPreview") }}
    </label>

    <span class="spacer" />

    <!-- Pause gates the scheduling loop: no new OCR/translation tasks while paused -->
    <button class="btn ghost" :class="{ warn: parse.paused }" :title="t('toolbar.pauseResumeTitle')" @click="parse.togglePaused">
      {{ parse.paused ? t("toolbar.startTranslation") : t("toolbar.pauseTranslation") }}
    </button>

    <span class="svc" :class="parse.serviceStatus" :title="t('toolbar.svcTitle')">
      <span class="svc-dot" />{{ svcText }}
    </span>

    <button class="icon-btn" :title="t('toolbar.settings')" @click="settings.openPage">
      <!-- Standard gear: the old "circle + rays" read as a light-mode sun icon -->
      <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <circle cx="12" cy="12" r="3" />
        <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
      </svg>
    </button>
  </header>
</template>

<style scoped>
.toolbar {
  height: var(--toolbar-h);
  flex: none;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 0 12px;
  background: var(--bg-panel);
  border-bottom: 1px solid var(--border);
  position: relative;
  z-index: 20; /* above the settings page (z-10): toolbar stays visible/clickable while settings is open */
}
.divider {
  width: 1px;
  height: 20px;
  background: var(--border);
  flex: none;
}
.spacer {
  flex: 1;
}

.seg {
  display: flex;
  background: var(--bg-hover);
  border-radius: var(--radius-sm);
  padding: 2px;
  gap: 2px;
}
.seg-btn {
  border: none;
  background: transparent;
  padding: 3px 10px;
  border-radius: 5px;
  cursor: pointer;
  color: var(--text-2);
  font-size: 12px;
  white-space: nowrap;
}
.seg-btn:hover {
  color: var(--text-1);
}
.seg-btn.active {
  background: var(--bg-panel);
  color: var(--accent);
  font-weight: 600;
  box-shadow: var(--shadow-1);
}

.zoom {
  display: flex;
  align-items: center;
  gap: 2px;
}
.fit-btn.active {
  color: var(--accent);
  background: var(--accent-weak);
}
.zoom-val {
  min-width: 46px;
  border: none;
  background: transparent;
  cursor: pointer;
  border-radius: var(--radius-sm);
  padding: 3px 4px;
  font-size: 12px;
  color: var(--text-2);
  text-align: center;
}
.zoom-val:hover {
  background: var(--bg-hover);
  color: var(--text-1);
}

.pagenav {
  display: flex;
  align-items: center;
  gap: 2px;
}
.page-ind {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 12px;
  color: var(--text-2);
}
.page-input {
  width: 44px;
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  background: var(--bg-panel);
  padding: 2px 6px;
  font-size: 12px;
  text-align: center;
  -moz-appearance: textfield;
  appearance: textfield;
}
.page-input::-webkit-outer-spin-button,
.page-input::-webkit-inner-spin-button {
  -webkit-appearance: none;
  margin: 0;
}
.page-total {
  white-space: nowrap;
}
</style>
