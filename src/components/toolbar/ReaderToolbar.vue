<script setup lang="ts">
import { computed } from "vue";
import { useLibraryStore } from "../../stores/library";
import { useParseStore } from "../../stores/parse";
import { useReaderStore } from "../../stores/reader";
import { useSettingsStore } from "../../stores/settings";
import { toast } from "../../composables/toast";
import type { LayoutMode } from "../../types/domain";

const lib = useLibraryStore();
const parse = useParseStore();
const reader = useReaderStore();
const settings = useSettingsStore();

const hasBook = computed(() => !!lib.currentBook);

const layoutOptions: Array<{ value: LayoutMode; label: string; title: string }> = [
  { value: "ot", label: "原│译", title: "左原文 · 右译文" },
  { value: "to", label: "译│原", title: "左译文 · 右原文" },
  { value: "o", label: "原文", title: "仅显示原文" },
  { value: "t", label: "译文", title: "仅显示译文" },
];

const svcText = computed(
  () =>
    ({
      unknown: "未连接",
      starting: "启动中",
      connected: "已连接",
      disconnected: "已断开",
    })[parse.serviceStatus],
);

function onPageInput(e: Event): void {
  const v = Number((e.target as HTMLInputElement).value);
  if (Number.isFinite(v) && v > 0) reader.gotoPage(v);
}

function onRetryPage(): void {
  toast("阶段4接入：本页重新入队解析");
}
</script>

<template>
  <header class="toolbar">
    <!-- 布局切换 -->
    <div class="seg" role="group" aria-label="阅读器布局">
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

    <!-- 缩放 -->
    <div class="zoom">
      <button class="icon-btn" title="缩小" @click="reader.zoomOut">−</button>
      <button class="zoom-val" title="重置缩放" @click="reader.resetZoom">{{ Math.round(reader.zoom * 100) }}%</button>
      <button class="icon-btn" title="放大" @click="reader.zoomIn">＋</button>
    </div>

    <span class="divider" />

    <!-- 页码导航 -->
    <div class="pagenav">
      <button class="icon-btn" title="上一页" :disabled="!hasBook || reader.currentPage <= 1" @click="reader.stepPage(-1)">‹</button>
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
      <button class="icon-btn" title="下一页" :disabled="!hasBook || reader.currentPage >= reader.pageCount" @click="reader.stepPage(1)">›</button>
    </div>

    <span class="divider" />

    <!-- 悬浮预览开关 -->
    <label class="switch" title="在原文文本块上悬浮显示译文">
      <input v-model="reader.hoverPreview" type="checkbox" />
      <span class="track"><span class="thumb" /></span>
      悬浮预览
    </label>

    <!-- 本页重解析 -->
    <button class="btn ghost" :disabled="!hasBook" title="把当前页重新入队解析" @click="onRetryPage">本页重解析</button>

    <span class="spacer" />

    <!-- 暂停/恢复解析 -->
    <button class="btn ghost" :class="{ warn: parse.paused }" @click="parse.togglePaused">
      {{ parse.paused ? "▶ 恢复解析" : "⏸ 暂停解析" }}
    </button>

    <!-- OCR 服务状态 -->
    <span class="svc" :class="parse.serviceStatus" title="OCR 服务连接状态">
      <span class="svc-dot" />{{ svcText }}
    </span>

    <button class="icon-btn" title="设置" @click="settings.openModal">
      <svg viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.3">
        <circle cx="8" cy="8" r="2.2" />
        <path d="M8 1.8v2M8 12.2v2M1.8 8h2M12.2 8h2M3.6 3.6l1.4 1.4M11 11l1.4 1.4M12.4 3.6L11 5M5 11l-1.4 1.4" />
      </svg>
    </button>
  </header>
</template>

<style scoped>
.toolbar {
  height: 46px;
  flex: none;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 0 12px;
  background: var(--bg-panel);
  border-bottom: 1px solid var(--border);
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

/* 分段按钮 */
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

/* 缩放 */
.zoom {
  display: flex;
  align-items: center;
  gap: 2px;
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

/* 页码 */
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
