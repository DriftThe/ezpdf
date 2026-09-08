<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { useLibraryStore } from "../../stores/library";
import { useReaderStore } from "../../stores/reader";
import PageCard from "./PageCard.vue";

/**
 * 单侧阅读栏：滚动容器 + 页面列。
 * 阶段2起 PageCard 内部换为 pdfjs canvas；本组件的滚动/同步协议保持不变。
 */
const props = defineProps<{
  kind: "original" | "translation";
}>();

const emit = defineEmits<{
  scrollRatio: [ratio: number];
  pageVisible: [page: number];
}>();

const lib = useLibraryStore();
const reader = useReaderStore();
const pages = computed(() => lib.currentBook?.pages ?? []);

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
    const max = el.scrollHeight - el.clientHeight;
    emit("scrollRatio", max > 0 ? el.scrollTop / max : 0);
    emit("pageVisible", visiblePage(el));
  });
}

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
      <span class="pane-title">{{ kind === "original" ? "原文" : "译文" }}</span>
      <span class="pane-hint">{{ kind === "original" ? "虚线框为 OCR 提取块" : "译文逐块覆盖渲染" }}</span>
    </header>
    <div ref="scrollEl" class="pane-scroll" @scroll="onScroll">
      <div class="page-col">
        <PageCard v-for="p in pages" :key="p.index" :page="p" :kind="kind" :zoom="reader.effectiveZoom" />
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
