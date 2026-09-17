<script setup lang="ts">
import { computed, onBeforeUnmount, ref, shallowRef, watch } from "vue";
import { useI18n } from "vue-i18n";
import type { PDFDocumentProxy } from "pdfjs-dist";
import { useLibraryStore } from "../../stores/library";
import { useReaderStore } from "../../stores/reader";
import { loadPdfDoc, destroyPdfDoc } from "../../composables/usePdfDoc";
import ReaderPane from "./ReaderPane.vue";
import EmptyState from "../common/EmptyState.vue";

/**
 * Reader area: arranges 1–2 ReaderPanes for the 4 layout modes and handles scroll sync.
 * Sync strategy: ratio mapping (1:1 when both panes have identical page geometry).
 */
type PaneKind = "original" | "translation";
type Side = "left" | "right";

const lib = useLibraryStore();
const reader = useReaderStore();
const { t } = useI18n();

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

function onPageVisible(page: number): void {
  reader.setVisiblePage(page);
}

/** PDF not yet being parsed (bound JSON missing or status Pending): the translation pane shows EmptyState, the original pane still shows blank pages */
function isTranslationPending(kind: PaneKind): boolean {
  return kind === "translation" && (lib.currentPdf?.bind === null || lib.currentPdf?.status === "Pending");
}

// ---- pdfjs document lifecycle: this component is the sole owner, both panes share one doc ----
const pdfDoc = shallowRef<PDFDocumentProxy | null>(null);
const docState = ref<"idle" | "loading" | "ready" | "error">("idle");
const docError = ref("");

watch(
  () => lib.currentPdfId,
  (id, oldId) => {
    void openDocument(id, oldId);
  },
);

/** Switch book: destroy old doc → load new doc → report geometry → measure per-page sizes in
 *  the background (re-check the id across awaits, discard race results) */
async function openDocument(id: string | null, oldId: string | null): Promise<void> {
  if (oldId) void destroyPdfDoc(oldId);
  pdfDoc.value = null;
  docError.value = "";
  reader.setPdfGeometry(0, 0, 0); // book switch: stale geometry invalid (pageCount falls back to the bound JSON)
  reader.setPageSizes([]); // per-page sizes invalid too
  const path = lib.currentPdf?.pdfPath;
  if (!id || !path) {
    docState.value = "idle";
    return;
  }
  docState.value = "loading";
  try {
    // pin: the focused book's doc is not LRU-evicted (background books in the scheduling bridge take at most 1 more)
    const doc = await loadPdfDoc(id, path, true);
    if (lib.currentPdfId !== id) {
      void destroyPdfDoc(id); // race: book switched away before load finished → discard
      return;
    }
    // Geometry report: real page count + page-1 size (pt; getViewport scale=1 gives 1pt=1px)
    const page1 = await doc.getPage(1);
    if (lib.currentPdfId !== id) {
      void destroyPdfDoc(id);
      return;
    }
    const vp = page1.getViewport({ scale: 1 });
    reader.setPdfGeometry(doc.numPages, vp.width, vp.height);
    // Re-arm the pending jump once geometry is ready: the jumpTarget set by restorePageFor is
    // consumed too early (page cards don't exist yet) before geometry, so reset it here to
    // restore the remembered reading position
    reader.jumpTarget = reader.currentPage;
    pdfDoc.value = doc;
    docState.value = "ready";
    void measurePageSizes(id, doc); // background per-page measurement (progressive; page-1 size holds until then)
  } catch (error) {
    if (lib.currentPdfId !== id) return; // book already switched, error no longer relevant
    docError.value = String(error);
    docState.value = "error";
  }
}

/**
 * Per-page measurement (background, batched concurrent): PDFs with mixed page sizes
 * (crops differ per scan page, plus landscape pages) require overlay positioning from
 * each page's true geometry — OCR's loc is converted from the same per-page viewport,
 * so both ends must agree. getPage is pure metadata parsing, no render cost.
 */
async function measurePageSizes(id: string, doc: PDFDocumentProxy): Promise<void> {
  const sizes: Array<{ w: number; h: number }> = new Array(doc.numPages);
  const CHUNK = 32;
  for (let i = 0; i < doc.numPages; i += CHUNK) {
    const end = Math.min(i + CHUNK, doc.numPages);
    const chunk = await Promise.all(
      Array.from({ length: end - i }, (_, k) =>
        doc.getPage(i + 1 + k).then((pg) => {
          const vp = pg.getViewport({ scale: 1 });
          return { w: vp.width, h: vp.height };
        }),
      ),
    );
    if (lib.currentPdfId !== id) return; // book already switched: stale measurement
    chunk.forEach((s, k) => {
      sizes[i + k] = s;
    });
    reader.setPageSizes([...sizes]); // progressive: measured pages immediately use their real size
  }
}

onBeforeUnmount(() => {
  const id = lib.currentPdfId;
  if (id) void destroyPdfDoc(id);
});

/** Toolbar/status-strip page jump, PDF switch position restore → sync-scroll both panes (manual scroll doesn't trigger).
 *  flush post: scroll after the DOM update, page cards only exist then (book switch/geometry ready) */
watch(
  () => reader.jumpTarget,
  (p) => {
    if (p == null) return;
    leftPane.value?.scrollToPage(p);
    rightPane.value?.scrollToPage(p);
    reader.jumpTarget = null;
  },
  { flush: "post" },
);
</script>

<template>
  <div class="reader-area">
    <template v-if="lib.currentPdf">
      <!-- Document-level load/error state (both panes are useless if fetching failed) -->
      <div v-if="docState === 'error'" class="pane-slot">
        <EmptyState :title="t('reader.loadFailed')" :desc="docError" />
      </div>
      <div v-else-if="docState === 'loading'" class="pane-slot">
        <EmptyState :title="t('common.loading')" :desc="t('reader.loadingDesc')" />
      </div>
      <template v-else>
        <!-- Left pane -->
        <div v-if="left && isTranslationPending(left)" class="pane-slot">
          <EmptyState :title="t('reader.notParsed')" :desc="t('reader.notParsedDesc')" />
        </div>
        <ReaderPane
          v-else-if="left"
          ref="leftPane"
          :kind="left"
          :doc="pdfDoc"
          @scroll-ratio="(r) => onScrollRatio('left', r)"
          @page-visible="onPageVisible"
        />
        <div v-if="right" class="pane-divider" />
        <!-- Right pane -->
        <div v-if="right && isTranslationPending(right)" class="pane-slot">
          <EmptyState :title="t('reader.notParsed')" :desc="t('reader.notParsedDesc')" />
        </div>
        <ReaderPane
          v-else-if="right"
          ref="rightPane"
          :kind="right"
          :doc="pdfDoc"
          @scroll-ratio="(r) => onScrollRatio('right', r)"
          @page-visible="onPageVisible"
        />
      </template>
    </template>
    <EmptyState v-else :title="t('reader.noPdf')" :desc="t('reader.noPdfDesc')">
      <button class="btn primary" :disabled="lib.importing" @click="lib.importPdf()">{{ t(lib.importing ? "reader.importing" : "reader.importPdf") }}</button>
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
