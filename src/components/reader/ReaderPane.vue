<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { PDFDocumentProxy } from "pdfjs-dist";
import { useLibraryStore } from "../../stores/library";
import { useReaderStore } from "../../stores/reader";
import type { Block } from "../../types/domain";
import PageCard from "./PageCard.vue";

/** One reader pane: scroll container + page column; page-list truth is reader.pageCount (pdfjs), virtualized by PdfPageCanvas. */
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

const pageNumbers = computed(() =>
  Array.from({ length: reader.pageCount }, (_, i) => i + 1),
);

const blocksByIndex = computed(() => {
  const map = new Map<number, Block[]>();
  for (const p of lib.currentPdf?.pages ?? []) map.set(p.index, p.blocks);
  return map;
});
const EMPTY_BLOCKS: Block[] = [];

const scrollEl = ref<HTMLElement | null>(null);
let raf = 0;

// Report clientWidth (excludes scrollbar); pane width comes from the flex layout, so no content feedback loop.
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
    if (restoring) return; // anchor restore is programmatic: don't report ratio (panes would push each other)
    const max = el.scrollHeight - el.clientHeight;
    emit("scrollRatio", max > 0 ? el.scrollTop / max : 0);
  });
}

// ---- Zoom/geometry anchor: native anchoring expires on width/height changes (scrollTop stays while content
// above scales → viewport drifts), so record "anchor page + in-page fraction" and restore after reflow ----
let anchor: { page: number; frac: number } | null = null;
let restoring = false;
let restoreFrames = 0;

/** Mark the restore window (covers the timing gap of scrollTop write → scroll event → rAF) */
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
  await nextTick(); // wait for the new size to land in the DOM (offsetTop/Height use the new geometry)
  const node = el.querySelector<HTMLElement>(`[data-page-index="${a.page - 1}"]`);
  if (!node) return;
  markRestoring();
  el.scrollTop = node.offsetTop + a.frac * node.offsetHeight;
}

// pre: capture the anchor on the old layout, then restore after Vue reflows
watch(
  [() => reader.effectiveZoom, () => reader.pageSizesPt, () => reader.pageCount],
  () => {
    if (reader.jumpTarget != null) return; // navigation jump takes priority
    captureAnchor();
    void restoreAnchor();
  },
  { flush: "pre" },
);

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
  /* Width comes only from the layout, not page content — otherwise fit-width and content size form a shrink loop */
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
  overflow: auto;
  background: var(--bg-workspace);
  overflow-anchor: none; /* native anchoring off: would fight the self-managed zoom anchor */
}
.page-col {
  position: relative; /* positioning base for page-card offsetTop */
  width: fit-content; /* wider than container: follow content so horizontal scroll reaches it */
  min-width: 100%; /* narrower than container: fill it so pages can center horizontally */
  min-height: 100%;
  padding: 14px 0;
  display: flex;
  flex-direction: column;
  align-items: center;
}
/* auto margins center when content is short; when it overflows they collapse to 0 and don't eat the scroll area */
.page-col > :first-child {
  margin-top: auto;
}
.page-col > :last-child {
  margin-bottom: auto;
}
</style>
