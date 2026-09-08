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
      <ReaderPane
        v-if="left"
        ref="leftPane"
        :kind="left"
        @scroll-ratio="(r) => onScrollRatio('left', r)"
        @page-visible="(p) => onPageVisible('left', p)"
      />
      <div v-if="right" class="pane-divider" />
      <ReaderPane
        v-if="right"
        ref="rightPane"
        :kind="right"
        @scroll-ratio="(r) => onScrollRatio('right', r)"
        @page-visible="(p) => onPageVisible('right', p)"
      />
    </template>
    <EmptyState v-else title="未打开任何书籍" desc="从左侧选择或导入一本 PDF 开始阅读">
      <button class="btn primary" @click="lib.importPdf">导入 PDF</button>
    </EmptyState>
  </div>
</template>

<style scoped>
.reader-area {
  flex: 1;
  min-height: 0;
  display: flex;
}
.pane-divider {
  width: 1px;
  background: var(--border);
  flex: none;
}
</style>
