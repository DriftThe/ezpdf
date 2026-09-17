<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from "vue";
import type { Directive } from "vue";
import type { PDFDocumentProxy } from "pdfjs-dist";
import { useReaderStore } from "../../stores/reader";
import { useSettingsStore } from "../../stores/settings";
import type { Block } from "../../types/domain";
import { isOverlayType } from "../../lib/blocks";
import { renderRichText } from "../../lib/richText";
import { parseTableMatrix, tableToText } from "../../lib/table";
import { fitFontSize } from "../../composables/fitFont";
import PdfPageCanvas from "./PdfPageCanvas.vue";
import TableCover from "./TableCover.vue";

/**
 * Single page card: pdfjs canvas (self-virtualized in PdfPageCanvas) + block overlay.
 * Card geometry = this page's real widthPt/heightPt × zoom (measured per page via
 * reader.pageSizeFor; a page-1 size assumption misaligns scan PDFs with mixed sizes).
 * Overlay system (absolute positioning, never reflows the original PDF):
 * - Original pane: dashed boxes for all blocks (controlled by the hover-preview toggle);
 * - Translation pane: a white cover box only for render types (lib/blocks.ts isOverlayType
 *   = user-checked types + formula), text = translation ?? content (during bypass
 *   translation is all null → shows the original content), font size auto-fitted by v-fit.
 */
const props = defineProps<{
  doc: PDFDocumentProxy | null;
  pageNumber: number;
  blocks: Block[];
  kind: "original" | "translation";
  zoom: number;
  widthPt: number;
  heightPt: number;
}>();

const reader = useReaderStore();

const width = computed(() => Math.round(props.widthPt * props.zoom));
const height = computed(() => Math.round(props.heightPt * props.zoom));

interface Rect {
  block: Block;
  left: number;
  top: number;
  width: number;
  height: number;
}

/** loc = [x1, y1, x2, y2] top-left → bottom-right corner → page-percentage rect */
const rects = computed<Rect[]>(() =>
  props.blocks.map((b) => ({
    block: b,
    left: (b.loc[0] / props.widthPt) * 100,
    top: (b.loc[1] / props.heightPt) * 100,
    width: ((b.loc[2] - b.loc[0]) / props.widthPt) * 100,
    height: ((b.loc[3] - b.loc[1]) / props.heightPt) * 100,
  })),
);

/** Original pane: block type → dashed box colour (image green, formula purple, table orange, title gets a bottom edge) */
const TYPE_CLASS: Record<string, string> = {
  image: "lbl-figure",
  formula: "lbl-formula",
  table: "lbl-table",
  paragraph_title: "lbl-title",
  doc_title: "lbl-title",
};

function typeClass(type: string): string {
  return TYPE_CLASS[type] ?? "lbl-text";
}

/** Percentage rect → absolute-position style (shared by original dashed box / translation white box) */
function rectStyle(r: Rect): Record<string, string> {
  return { left: r.left + "%", top: r.top + "%", width: r.width + "%", height: r.height + "%" };
}

interface CoverRect extends Rect {
  /** Content HTML: body escaped + formulas via KaTeX (lib/richText.ts) */
  html: string;
}

/** Translation covers: render types with non-empty text only; content becomes rich text (empty figure etc. draws no white box) */
const settings = useSettingsStore();
/** User-checked translate types (Settings → General); unchecked types are not covered, original PDF pixels show through */
const overlayTypes = computed(() => new Set(settings.general.translateTypes));

const coverRects = computed<CoverRect[]>(() =>
  rects.value
    .filter(
      (r) =>
        r.block.type !== "table" && // tables go through TableCover (grid render), not a text box
        isOverlayType(r.block.type, overlayTypes.value) &&
        (r.block.translation ?? r.block.content).trim(),
    )
    .map((r) => ({ ...r, html: renderRichText(r.block.translation ?? r.block.content) })),
);

/**
 * Table covers in the translation pane: drawn only when table is checked and Rust has
 * parsed a grid. Unchecked / not yet processed (no grid / parse failed) → not drawn,
 * original PDF pixels show through.
 */
const tableRects = computed<Rect[]>(() =>
  rects.value.filter(
    (r) => r.block.type === "table" && r.block.grid && isOverlayType(r.block.type, overlayTypes.value),
  ),
);

// ---- Original-pane hover preview: hovering a translated block shows a translation card
// (a floating layer, deliberately not a title attribute). Position follows the cursor
// (mousemove throttled via rAF, positioning only, no content re-render); in the lower
// viewport half it flips upward (above → translateY(-100%)), clamped horizontally.

interface HoverInfo {
  html: string;
  x: number;
  y: number;
  above: boolean;
}

const hoverInfo = ref<HoverInfo | null>(null);

/** Mouse position → card placement (12px right margin; flips up in the lower 45% of the viewport) */
function previewPos(e: MouseEvent): { x: number; y: number; above: boolean } {
  const cardMax = Math.min(460, window.innerWidth - 24);
  const x = Math.max(8, Math.min(e.clientX + 16, window.innerWidth - cardMax - 12));
  const above = e.clientY > window.innerHeight * 0.55;
  const y = above ? e.clientY - 12 : e.clientY + 16;
  return { x, y, above };
}

function onBlockEnter(r: Rect, e: MouseEvent): void {
  const translated = r.block.translation?.trim();
  if (!translated) return; // untranslated (including non-translate types): don't pop
  // Table: translation is a 2-D matrix JSON; convert to readable multi-line text (don't show raw JSON)
  const grid = r.block.grid;
  const html =
    r.block.type === "table" && grid
      ? renderRichText(tableToText(grid, parseTableMatrix(r.block.translation, grid)))
      : renderRichText(translated);
  hoverInfo.value = { html, ...previewPos(e) };
}

let moveRaf = 0;
let pendingMove: MouseEvent | null = null;

/** Follow the cursor: update position at most once per frame (html unchanged, Vue only patches style) */
function onBlockMove(e: MouseEvent): void {
  if (!hoverInfo.value) return;
  pendingMove = e;
  if (moveRaf) return;
  moveRaf = requestAnimationFrame(() => {
    moveRaf = 0;
    const ev = pendingMove;
    pendingMove = null;
    if (hoverInfo.value && ev) {
      hoverInfo.value = { ...hoverInfo.value, ...previewPos(ev) };
    }
  });
}

function onBlockLeave(): void {
  if (moveRaf) {
    cancelAnimationFrame(moveRaf);
    moveRaf = 0;
  }
  pendingMove = null;
  hoverInfo.value = null;
}

onBeforeUnmount(() => {
  if (moveRaf) cancelAnimationFrame(moveRaf);
});

// Close the preview when the toggle turns off: the indicator boxes disappear with it
watch(
  () => reader.hoverPreview,
  (on) => {
    if (!on) onBlockLeave();
  },
);

// ---- v-fit: cover-box font-size auto-fit (box size × text amount → fills without overflowing) ----
// Performance constraint (measured): resizing all cover boxes at once on zoom and running the
// binary search synchronously would block the main thread for seconds on large books. So measure
// only boxes near the viewport: off-screen boxes are merely marked dirty (WeakSet, zero layout)
// and fitted when they enter the viewport (IntersectionObserver, one screen of margin); updated /
// ResizeObserver also just mark off-screen boxes dirty.

const roMap = new WeakMap<HTMLElement, ResizeObserver>();
const visibleBoxes = new Set<HTMLElement>();
const dirtyBoxes = new WeakSet<HTMLElement>();
const pendingFits = new Set<HTMLElement>();
let flushRaf = 0;

// KaTeX font load completes = glyph widths/line heights changed: re-fit the currently
// visible boxes (small count, negligible cost). The listener is global while this code
// runs once per page-card instance — it must be removed on unmount, otherwise every page
// leaves a never-collected listener (its closure pins each instance's visibleBoxes).
const onFontsLoaded = (): void => {
  for (const box of visibleBoxes) fitCoverText(box);
};
if (typeof document !== "undefined" && "fonts" in document) {
  document.fonts.addEventListener("loadingdone", onFontsLoaded);
  onBeforeUnmount(() => document.fonts.removeEventListener("loadingdone", onFontsLoaded));
}

function fitCoverText(box: HTMLElement): void {
  const span = box.querySelector<HTMLElement>(".cover-text");
  if (!span) return;
  const cs = getComputedStyle(box);
  const inner =
    box.clientHeight - (parseFloat(cs.paddingTop) + parseFloat(cs.paddingBottom));
  if (inner <= 0) return;
  const MIN = 5;
  const size = fitFontSize(
    (s) => {
      span.style.fontSize = `${s}px`;
      return span.scrollHeight <= inner;
    },
    MIN,
    inner, // single-line cap = box content height (larger would overflow vertically)
  );
  span.style.fontSize = `${size}px`; // when even MIN overflows, stay at MIN and let overflow:hidden truncate
}

/** Request a fit: off-screen boxes just get marked dirty for the IO; visible boxes join the batch and are measured at frame end (updated/RO deduped within a frame) */
function requestFit(box: HTMLElement): void {
  if (!visibleBoxes.has(box)) {
    dirtyBoxes.add(box);
    return;
  }
  pendingFits.add(box);
  if (!flushRaf) flushRaf = requestAnimationFrame(flushFits);
}

function flushFits(): void {
  flushRaf = 0;
  for (const box of pendingFits) {
    if (visibleBoxes.has(box)) fitCoverText(box);
  }
  pendingFits.clear();
}

const fitIO = new IntersectionObserver(
  (entries) => {
    for (const e of entries) {
      const box = e.target as HTMLElement;
      if (!e.isIntersecting) {
        visibleBoxes.delete(box);
        continue;
      }
      visibleBoxes.add(box);
      if (dirtyBoxes.has(box)) {
        dirtyBoxes.delete(box);
        fitCoverText(box); // initial/entry report: fit before paint, avoiding a flash of the default size
      }
    }
  },
  { rootMargin: "50% 0px" }, // fit about half a screen before entry (canvas virtualization stays a full screen)
);

const vFit: Directive<HTMLElement> = {
  mounted(box) {
    dirtyBoxes.add(box); // don't measure now: wait for the first IO report, off-screen boxes pay no layout cost
    fitIO.observe(box);
    const ro = new ResizeObserver(() => requestFit(box)); // zoom/window change → re-fit
    ro.observe(box);
    roMap.set(box, ro);
  },
  updated(box) {
    requestFit(box); // re-fit after text/geometry updates
  },
  unmounted(box) {
    fitIO.unobserve(box);
    roMap.get(box)?.disconnect();
    roMap.delete(box);
    pendingFits.delete(box);
    visibleBoxes.delete(box);
    dirtyBoxes.delete(box);
  },
};
</script>

<template>
  <div class="page-wrap" :data-page-index="pageNumber - 1">
    <div class="page-card" :style="{ width: width + 'px', height: height + 'px' }">
      <!-- pdfjs canvas: same render in both panes, all differences live in the overlays -->
      <PdfPageCanvas :doc="doc" :page-number="pageNumber" :zoom="zoom" />

      <!-- Original: dashed OCR block boxes (also hides the hover targets when the preview is off);
           hovering a translated block pops the translation card (a floating layer, not a title) -->
      <template v-if="kind === 'original' && reader.hoverPreview">
        <div
          v-for="(r, ri) in rects"
          :key="ri"
          class="blk-line"
          :class="typeClass(r.block.type)"
          :style="rectStyle(r)"
          @mouseenter="onBlockEnter(r, $event)"
          @mousemove="onBlockMove"
          @mouseleave="onBlockLeave"
        />
      </template>

      <!-- Translation: white covers for render types only (uncovered types show original PDF pixels);
           box text = translation ?? content: during bypass all null → original content -->
      <template v-if="kind === 'translation'">
        <div
          v-for="(r, ri) in coverRects"
          :key="ri"
          v-fit
          class="blk-cover cover-box"
          :class="{ 'cover-formula': r.block.type === 'formula' }"
          :style="rectStyle(r)"
          :title="r.block.type"
        >
          <span class="cover-text" v-html="r.html"></span>
        </div>
        <!-- Table: redrawn as a web table (the component goes transparent when it doesn't fit, original PDF pixels show through) -->
        <TableCover
          v-for="(r, ri) in tableRects"
          :key="`t${ri}`"
          :grid="r.block.grid!"
          :translation="r.block.translation"
          :style="rectStyle(r)"
        />
      </template>
    </div>
    <div class="page-num">{{ pageNumber }}</div>
  </div>

  <!-- Original-pane hover translation card: teleported to body (escapes the scroll container clip), fixed to follow the cursor -->
  <Teleport to="body">
    <div
      v-if="hoverInfo"
      class="hover-preview"
      :class="{ above: hoverInfo.above }"
      :style="{ left: hoverInfo.x + 'px', top: hoverInfo.y + 'px' }"
      v-html="hoverInfo.html"
    />
  </Teleport>
</template>

<style scoped>
.page-wrap {
  display: flex;
  flex-direction: column;
  align-items: center;
}
.page-card {
  position: relative;
  background: var(--page-bg);
  border-radius: 2px;
  box-shadow: var(--shadow-1);
  flex: none;
}
.page-num {
  font-size: 11px;
  color: var(--text-3);
  padding: 5px 0 14px;
  user-select: none;
}

/* ---- Original: dashed boxes (page is always white, so annotation colours are theme-independent) ---- */
.blk-line {
  position: absolute;
  border: 1px dashed var(--page-annot);
  border-radius: 2px;
  opacity: 0.55;
  pointer-events: auto;
}
.blk-line:hover {
  opacity: 1;
  background: var(--page-annot-weak);
}
.blk-line.lbl-figure {
  border-color: var(--page-annot-figure);
}
.blk-line.lbl-formula {
  border-color: var(--page-annot-formula);
}
.blk-line.lbl-table {
  border-color: var(--page-annot-table);
}
.blk-line.lbl-title {
  border-color: var(--page-annot);
  border-bottom-style: solid;
}

/* ---- Original-pane hover translation card (teleported to body): white card follows the cursor, doesn't capture the mouse ---- */
.hover-preview {
  position: fixed;
  z-index: 100;
  max-width: min(460px, calc(100vw - 24px));
  padding: 8px 10px;
  background: var(--bg-panel);
  color: var(--text-1);
  border: 1px solid var(--border);
  border-radius: 6px;
  box-shadow: 0 6px 24px rgba(0, 0, 0, 0.18);
  font-size: 13px;
  line-height: 1.55;
  overflow-wrap: anywhere;
  pointer-events: none; /* display only: don't steal mouse events from the original blocks */
}
.hover-preview.above {
  transform: translateY(-100%); /* flip up in the lower viewport half to avoid clipping */
}
.hover-preview :deep(.katex-display) {
  margin: 0.1em 0;
}

/* ---- Translation: white covers (unchecked types get no cover) ---- */
/* Base styles in main.css .cover-box (shared with TableCover) */
.blk-cover {
  padding: 0;
}
/* Formula: KaTeX vertically centered; KaTeX display formulas carry 1em top/bottom margins,
   which in a ~16pt-high box push content to the top and force v-fit to pick a tiny size — trim them */
.cover-formula {
  display: flex;
  align-items: center;
}
.cover-formula :deep(.katex-display) {
  margin: 0.15em 0;
}
.cover-text {
  display: block;
  width: 100%;
  line-height: 1.25;
  color: #222;
  /* font size is set dynamically by the v-fit directive from box size × text amount */
  overflow-wrap: anywhere; /* long words/URLs also wrap, width never overflows */
}
</style>
