<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { PDFDocumentProxy } from "pdfjs-dist";
import { useLibraryStore } from "../../stores/library";
import { useReaderStore } from "../../stores/reader";
import type { Block } from "../../types/domain";
import PageCard from "./PageCard.vue";

/**
 * 单侧阅读栏：滚动容器 + 页面列。
 * 页列表真相源 = pdfjs 实测页数（reader.pageCount）；绑定 JSON 只供 OCR 块数据。
 * 虚拟化由 PdfPageCanvas 自管（IntersectionObserver），本组件滚动/同步协议不变。
 */
const props = defineProps<{
  kind: "original" | "translation";
  doc: PDFDocumentProxy | null;
}>();

const emit = defineEmits<{
  scrollRatio: [ratio: number];
  pageVisible: [page: number];
}>();

const lib = useLibraryStore();
const reader = useReaderStore();
const { t } = useI18n();

/** 渲染页列表：1..numPages（Pending 书也有全部页卡，块覆盖层为空） */
const pageNumbers = computed(() =>
  Array.from({ length: reader.pageCount }, (_, i) => i + 1),
);

/** 绑定 JSON 的 OCR 块按 1-based 页号查表；查不到的页（未解析）空覆盖层 */
const blocksByIndex = computed(() => {
  const map = new Map<number, Block[]>();
  for (const p of lib.currentPdf?.pages ?? []) map.set(p.index, p.blocks);
  return map;
});
const EMPTY_BLOCKS: Block[] = [];

const scrollEl = ref<HTMLElement | null>(null);
let raf = 0;

// 向 store 上报栏宽（供适应宽度实时计算缩放；窗口尺寸/布局切换/侧栏收展时自动重算）。
// 用 clientWidth（排除滚动条）而非 contentRect，且栏宽由 flex 布局决定、与页面内容无关，避免反馈循环。
let ro: ResizeObserver | null = null;
onMounted(() => {
  if (!scrollEl.value) return;
  ro = new ResizeObserver(() => {
    if (scrollEl.value) reader.setPaneWidth(scrollEl.value.clientWidth);
  });
  ro.observe(scrollEl.value);
});
onBeforeUnmount(() => {
  ro?.disconnect();
  ro = null;
});

function onScroll(e: Event): void {
  const el = e.target as HTMLElement;
  cancelAnimationFrame(raf);
  raf = requestAnimationFrame(() => {
    emit("pageVisible", visiblePage(el));
    if (restoring) return; // 锚点恢复的程序化滚动：不上报比例，避免双栏互推
    const max = el.scrollHeight - el.clientHeight;
    emit("scrollRatio", max > 0 ? el.scrollTop / max : 0);
  });
}

// ---- 缩放/几何变化锚点（用户 2026-09-14）：zoom 变化让全部页卡同步伸缩，浏览器原生
// scroll anchoring 对 width/height 变化主动失效 → scrollTop 不变而上方内容已按比例缩放，
// 视口漂移（页数越多越明显；逐页几何后台回填同理）。这里自己记「视口内锚点页 + 页内比例」，
// 重排后恢复同一内容位置。导航跳页（jumpTarget）期间不抢滚动。 ----
let anchor: { page: number; frac: number } | null = null;
let restoring = false;
let restoreFrames = 0;

/** 标记恢复窗口（覆盖「scrollTop 写入 → scroll 事件 → rAF」的时机差） */
function markRestoring(): void {
  restoring = true;
  restoreFrames = 2;
  const tick = (): void => {
    if (--restoreFrames <= 0) {
      restoring = false;
      return;
    }
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

function captureAnchor(): void {
  const el = scrollEl.value;
  if (!el) return;
  const nodes = el.querySelectorAll<HTMLElement>("[data-page-index]");
  const center = el.scrollTop + el.clientHeight / 2;
  let best: HTMLElement | null = null;
  let bestDist = Number.POSITIVE_INFINITY;
  nodes.forEach((n) => {
    const top = n.offsetTop;
    const bottom = top + n.offsetHeight;
    const d = center < top ? top - center : center > bottom ? center - bottom : 0;
    if (d < bestDist) {
      bestDist = d;
      best = n;
    }
  });
  const node = best as HTMLElement | null;
  if (!node) {
    anchor = null;
    return;
  }
  anchor = {
    page: Number(node.dataset.pageIndex) + 1,
    frac: node.offsetHeight > 0 ? (el.scrollTop - node.offsetTop) / node.offsetHeight : 0,
  };
}

async function restoreAnchor(): Promise<void> {
  const el = scrollEl.value;
  const a = anchor;
  anchor = null;
  if (!el || !a) return;
  await nextTick(); // 等新尺寸落到 DOM（offsetTop/Height 已按新几何）
  const node = el.querySelector<HTMLElement>(`[data-page-index="${a.page - 1}"]`);
  if (!node) return;
  markRestoring();
  el.scrollTop = node.offsetTop + a.frac * node.offsetHeight;
}

// pre：先在旧布局上采锚点，再等 Vue 重排后恢复
watch(
  [() => reader.effectiveZoom, () => reader.pageSizesPt, () => reader.pageCount],
  () => {
    if (reader.jumpTarget != null) return; // 导航跳页优先
    captureAnchor();
    void restoreAnchor();
  },
  { flush: "pre" },
);

/** 以滚动容器垂直中点所在页为可视页 */
function visiblePage(el: HTMLElement): number {
  const center = el.scrollTop + el.clientHeight / 2;
  const nodes = el.querySelectorAll<HTMLElement>("[data-page-index]");
  let best = 0;
  let bestDist = Number.POSITIVE_INFINITY;
  nodes.forEach((n) => {
    const top = n.offsetTop;
    const bottom = top + n.offsetHeight;
    const d = center < top ? top - center : center > bottom ? center - bottom : 0;
    if (d < bestDist) {
      bestDist = d;
      best = Number(n.dataset.pageIndex);
    }
  });
  return best + 1;
}

function scrollToRatio(ratio: number): void {
  const el = scrollEl.value;
  if (!el) return;
  el.scrollTop = ratio * (el.scrollHeight - el.clientHeight);
}

function scrollToPage(page: number): void {
  const el = scrollEl.value;
  if (!el) return;
  const target = el.querySelector<HTMLElement>(`[data-page-index="${page - 1}"]`);
  if (target) el.scrollTop = target.offsetTop - 14;
}

defineExpose({ scrollToRatio, scrollToPage });
</script>

<template>
  <section class="pane" :class="kind">
    <header class="pane-head">
      <span class="pane-title">{{ t(kind === "original" ? "reader.original" : "reader.translation") }}</span>
      <span class="pane-hint">{{ t(kind === "original" ? "reader.originalHint" : "reader.translationHint") }}</span>
    </header>
    <div ref="scrollEl" class="pane-scroll" @scroll="onScroll">
      <div class="page-col">
        <PageCard
          v-for="n in pageNumbers"
          :key="n"
          :page-number="n"
          :blocks="blocksByIndex.get(n) ?? EMPTY_BLOCKS"
          :kind="kind"
          :doc="doc"
          :zoom="reader.effectiveZoom"
          :width-pt="reader.pageSizeFor(n).w"
          :height-pt="reader.pageSizeFor(n).h"
        />
      </div>
    </div>
  </section>
</template>

<style scoped>
.pane {
  /* 剩余宽度平均分给两栏（单栏时独占）；宽度只由布局决定，
     不随页面内容伸缩 —— 否则适应宽度会与内容尺寸形成收缩循环 */
  flex: 1 1 0;
  display: flex;
  flex-direction: column;
  height: 100%;
  min-width: 0;
}
.pane-head {
  height: 30px;
  flex: none;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 0 12px;
  background: var(--bg-panel);
  border-bottom: 1px solid var(--border);
  user-select: none;
}
.pane-title {
  font-size: 12px;
  font-weight: 600;
  color: var(--text-1);
}
.pane-hint {
  font-size: 11px;
  color: var(--text-3);
}
.pane-scroll {
  flex: 1;
  overflow: auto; /* 适应宽度时无水平溢出；手动放大后允许水平滚动 */
  background: var(--bg-workspace);
  overflow-anchor: none; /* 锚点由本组件自管（缩放锚点），关掉原生双保险避免互相打架 */
}
.page-col {
  position: relative; /* 页卡 offsetTop 的定位基准 */
  width: fit-content; /* 内容宽于容器时按内容走，保证水平滚动可达 */
  min-width: 100%; /* 内容窄于容器时撑满，页面可水平居中 */
  min-height: 100%;
  padding: 14px 0;
  display: flex;
  flex-direction: column;
  align-items: center;
}
/* 内容总高小于容器时垂直居中；超出时 auto margin 归零，不吃掉可滚动区 */
.page-col > :first-child {
  margin-top: auto;
}
.page-col > :last-child {
  margin-bottom: auto;
}
</style>
