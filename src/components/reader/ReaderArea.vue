<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useLibraryStore } from "../../stores/library";
import { useReaderStore } from "../../stores/reader";
import ReaderPane from "./ReaderPane.vue";
import EmptyState from "../common/EmptyState.vue";

/**
 * 阅读区：按布局 4 态编排 1–2 个 ReaderPane，并负责滚动同步。
 * 同步策略：比例映射（两栏页面几何一致时即 1:1 对应）。
 */
type PaneKind = "original" | "translation";
type Side = "left" | "right";

const lib = useLibraryStore();
const reader = useReaderStore();

const left = computed<PaneKind | null>(() => {
  switch (reader.layout) {
    case "ot":
    case "o":
      return "original";
    case "to":
    case "t":
      return "translation";
  }
});
const right = computed<PaneKind | null>(() => {
  switch (reader.layout) {
    case "ot":
      return "translation";
    case "to":
      return "original";
    default:
      return null;
  }
});

const leftPane = ref<InstanceType<typeof ReaderPane> | null>(null);
const rightPane = ref<InstanceType<typeof ReaderPane> | null>(null);
let syncing = false;

function onScrollRatio(side: Side, ratio: number): void {
  if (syncing || !right.value) return;
  syncing = true;
  (side === "left" ? rightPane : leftPane).value?.scrollToRatio(ratio);
  requestAnimationFrame(() => {
    syncing = false;
  });
}

function onPageVisible(_side: Side, page: number): void {
  reader.setVisiblePage(page);
}

/** 未解析的书（结构 JSON 未生成，bind 为 null）：译文视窗用 EmptyState 提示，原文视窗仍显示空白页 */
function isTranslationPending(kind: PaneKind): boolean {
  return kind === "translation" && lib.currentBook?.bind === null;
}

/** 工具栏/状态条跳页、切书恢复位置 → 两栏同步滚动（手动滚动不触发） */
watch(
  () => reader.jumpTarget,
  (p) => {
    if (p == null) return;
    leftPane.value?.scrollToPage(p);
    rightPane.value?.scrollToPage(p);
    reader.jumpTarget = null;
  },
);
</script>

<template>
  <div class="reader-area">
    <template v-if="lib.currentBook">
      <!-- 左栏 -->
      <div v-if="left && isTranslationPending(left)" class="pane-slot">
        <EmptyState title="该文件还未解析" desc="结构 JSON 尚未生成，解析完成后此处将渲染译文" />
      </div>
      <ReaderPane
        v-else-if="left"
        ref="leftPane"
        :kind="left"
        @scroll-ratio="(r) => onScrollRatio('left', r)"
        @page-visible="(p) => onPageVisible('left', p)"
      />
      <div v-if="right" class="pane-divider" />
      <!-- 右栏 -->
      <div v-if="right && isTranslationPending(right)" class="pane-slot">
        <EmptyState title="该文件还未解析" desc="结构 JSON 尚未生成，解析完成后此处将渲染译文" />
      </div>
      <ReaderPane
        v-else-if="right"
        ref="rightPane"
        :kind="right"
        @scroll-ratio="(r) => onScrollRatio('right', r)"
        @page-visible="(p) => onPageVisible('right', p)"
      />
    </template>
    <EmptyState v-else title="未打开任何PDF" desc="从左侧选择或导入一份 PDF 开始阅读">
      <button class="btn primary" @click="lib.importPdf">导入 PDF</button>
    </EmptyState>
  </div>
</template>

<style scoped>
.reader-area {
  flex: 1;
  min-height: 0;
  display: flex;
  justify-content: center;
}
.pane-slot {
  flex: 1 1 0;
  min-width: 0;
  display: flex;
  flex-direction: column;
  background: var(--bg-workspace);
}
.pane-divider {
  width: 1px;
  background: var(--border);
  flex: none;
}
</style>
