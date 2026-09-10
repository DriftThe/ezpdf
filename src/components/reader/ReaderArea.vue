<script setup lang="ts">
import { computed, onBeforeUnmount, ref, shallowRef, watch } from "vue";
import type { PDFDocumentProxy } from "pdfjs-dist";
import { useLibraryStore } from "../../stores/library";
import { useReaderStore } from "../../stores/reader";
import { loadPdfDoc, destroyPdfDoc } from "../../composables/usePdfDoc";
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

/** 未开始解析的 PDF（绑定 JSON 缺失或 status 为 Pending）：译文视窗用 EmptyState 提示，原文视窗仍显示空白页 */
function isTranslationPending(kind: PaneKind): boolean {
  return kind === "translation" && (lib.currentPdf?.bind === null || lib.currentPdf?.status === "Pending");
}

// ---- pdfjs 文档生命周期（阶段2）：唯一持有者在本组件，两栏共用同一 doc ----
const pdfDoc = shallowRef<PDFDocumentProxy | null>(null);
const docState = ref<"idle" | "loading" | "ready" | "error">("idle");
const docError = ref("");

watch(
  () => lib.currentPdfId,
  async (id, oldId) => {
    if (oldId) void destroyPdfDoc(oldId);
    pdfDoc.value = null;
    docError.value = "";
    reader.setPdfGeometry(0, 0, 0); // 切书：旧几何失效（pageCount 回退绑定 JSON）
    const path = lib.currentPdf?.pdfPath;
    if (!id || !path) {
      docState.value = "idle";
      return;
    }
    docState.value = "loading";
    try {
      const doc = await loadPdfDoc(id, path);
      if (lib.currentPdfId !== id) {
        void destroyPdfDoc(id); // 竞态：加载完成时书已切走 → 丢弃
        return;
      }
      // 几何上报：真实页数 + 第 1 页尺寸（pt，getViewport scale=1 时 1pt=1px）
      const page1 = await doc.getPage(1);
      if (lib.currentPdfId !== id) {
        void destroyPdfDoc(id);
        return;
      }
      const vp = page1.getViewport({ scale: 1 });
      reader.setPdfGeometry(doc.numPages, vp.width, vp.height);
      pdfDoc.value = doc;
      docState.value = "ready";
    } catch (error) {
      if (lib.currentPdfId !== id) return; // 已切书，错误不再相关
      docError.value = String(error);
      docState.value = "error";
    }
  },
);

onBeforeUnmount(() => {
  const id = lib.currentPdfId;
  if (id) void destroyPdfDoc(id);
});

/** 工具栏/状态条跳页、切换 PDF 恢复位置 → 两栏同步滚动（手动滚动不触发） */
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
    <template v-if="lib.currentPdf">
      <!-- 文档级加载/错误态（取数失败两栏都无事可做） -->
      <div v-if="docState === 'error'" class="pane-slot">
        <EmptyState title="PDF 加载失败" :desc="docError" />
      </div>
      <div v-else-if="docState === 'loading'" class="pane-slot">
        <EmptyState title="加载中…" desc="正在读取 PDF 文件" />
      </div>
      <template v-else>
        <!-- 左栏 -->
        <div v-if="left && isTranslationPending(left)" class="pane-slot">
          <EmptyState title="该文件还未解析" desc="解析完成后此处将渲染译文" />
        </div>
        <ReaderPane
          v-else-if="left"
          ref="leftPane"
          :kind="left"
          :doc="pdfDoc"
          :width-pt="reader.pageSizePt.w"
          :height-pt="reader.pageSizePt.h"
          @scroll-ratio="(r) => onScrollRatio('left', r)"
          @page-visible="(p) => onPageVisible('left', p)"
        />
        <div v-if="right" class="pane-divider" />
        <!-- 右栏 -->
        <div v-if="right && isTranslationPending(right)" class="pane-slot">
          <EmptyState title="该文件还未解析" desc="解析完成后此处将渲染译文" />
        </div>
        <ReaderPane
          v-else-if="right"
          ref="rightPane"
          :kind="right"
          :doc="pdfDoc"
          :width-pt="reader.pageSizePt.w"
          :height-pt="reader.pageSizePt.h"
          @scroll-ratio="(r) => onScrollRatio('right', r)"
          @page-visible="(p) => onPageVisible('right', p)"
        />
      </template>
    </template>
    <EmptyState v-else title="未打开任何PDF" desc="从左侧选择或导入一份 PDF 开始阅读">
      <button class="btn primary" :disabled="lib.importing" @click="lib.importPdf()">{{ lib.importing ? "导入中" : "导入 PDF" }}</button>
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
