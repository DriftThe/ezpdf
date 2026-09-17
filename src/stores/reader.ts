import { defineStore } from "pinia";
import { computed, ref } from "vue";
import type { LayoutMode } from "../types/domain";
import { useLibraryStore } from "./library";

const MIN_ZOOM = 0.1;
const MAX_ZOOM = 4;
/** Total horizontal page padding in fit-width mode (px). */
const PAGE_FIT_PADDING = 32;

export const useReaderStore = defineStore("reader", () => {
  const layout = ref<LayoutMode>("ot");
  /** width = fit pane (default, no h-scroll); custom = manual zoom. */
  const fitMode = ref<"width" | "custom">("width");
  /** Zoom under custom mode (pt→px factor, 1 = original size). */
  const zoom = ref(1);
  /** Scroll container width reported by ReaderPane (px, scrollbar excluded). */
  const paneWidth = ref(0);
  /** 1-based current page. */
  const currentPage = ref(1);
  /** Hover translation toggle (also hides the original-pane indicator rects). */
  const hoverPreview = ref(true);
  /** Remembered reading position per PDF. */
  const pageByPdf = ref<Record<string, number>>({});
  /** Pending jump target (set by gotoPage/PDF switch, cleared by ReaderArea; manual scroll leaves it alone). */
  const jumpTarget = ref<number | null>(null);

  /** pdfjs geometry (render source of truth): real page count + page-1 size (pt).
   *  0 until the doc is ready; pageCount then falls back to the bound JSON. */
  const numPages = ref(0);
  const pageSizePt = ref({ w: 0, h: 0 });
  /** Per-page measured size (pt) — overlay alignment for PDFs with varying page sizes.
   *  Missing entries fall back to page 1. */
  const pageSizesPt = ref<Array<{ w: number; h: number }>>([]);

  /** Reported by the render layer once ready; the caller zeroes it on a book switch. */
  function setPdfGeometry(pages: number, w: number, h: number): void {
    numPages.value = pages;
    pageSizePt.value = { w, h };
  }

  /** Batch per-page sizes (measured progressively after the doc is ready; zeroed on switch). */
  function setPageSizes(sizes: Array<{ w: number; h: number }>): void {
    pageSizesPt.value = sizes;
  }

  /** Real size of 1-based page n: per-page measurement first, else page 1. */
  function pageSizeFor(n: number): { w: number; h: number } {
    return pageSizesPt.value[n - 1] ?? pageSizePt.value;
  }

  /** Page count: pdfjs first, else the bound JSON page list. */
  const pageCount = computed(() =>
    numPages.value > 0 ? numPages.value : (useLibraryStore().currentPdf?.pages.length ?? 0),
  );
  /** Effective zoom: (pane − padding) / page width in pt, recomputed live. */
  const effectiveZoom = computed(() => {
    if (fitMode.value !== "width" || paneWidth.value <= 0) return zoom.value;
    const pw = pageSizePt.value.w;
    if (pw <= 0) return zoom.value;
    return clampZoom((paneWidth.value - PAGE_FIT_PADDING) / pw);
  });

  function setLayout(l: LayoutMode): void {
    layout.value = l;
  }
  function setPaneWidth(w: number): void {
    paneWidth.value = w;
  }

  function zoomIn(): void {
    leaveFit();
    zoom.value = clampZoom(zoom.value * 1.15);
  }
  function zoomOut(): void {
    leaveFit();
    zoom.value = clampZoom(zoom.value / 1.15);
  }
  /** Toolbar percentage button: fit-width ↔ 100%. */
  function toggleFit(): void {
    if (fitMode.value === "width") {
      fitMode.value = "custom";
      zoom.value = 1;
    } else {
      fitMode.value = "width";
    }
  }
  /** Leave fit mode before manual zoom, starting from the current effective zoom. */
  function leaveFit(): void {
    if (fitMode.value !== "width") return;
    const cur = effectiveZoom.value;
    fitMode.value = "custom";
    zoom.value = cur;
  }

  function gotoPage(p: number): void {
    const max = pageCount.value || 1;
    currentPage.value = Math.max(1, Math.min(max, Math.floor(p)));
    rememberCurrent();
    jumpTarget.value = currentPage.value;
  }
  function stepPage(delta: number): void {
    gotoPage(currentPage.value + delta);
  }
  /** Visible page reported by ReaderArea while scrolling (does not scroll back). */
  function setVisiblePage(p: number): void {
    if (p >= 1 && p <= (pageCount.value || 1) && p !== currentPage.value) {
      currentPage.value = p;
      rememberCurrent();
    }
  }
  /** Restore the last reading position on a PDF switch. */
  function restorePageFor(pdfId: string): void {
    currentPage.value = pageByPdf.value[pdfId] ?? 1;
    jumpTarget.value = currentPage.value;
  }
  function rememberCurrent(): void {
    const lib = useLibraryStore();
    if (lib.currentPdfId) {
      pageByPdf.value[lib.currentPdfId] = currentPage.value;
    }
  }

  return {
    layout,
    fitMode,
    currentPage,
    hoverPreview,
    pageCount,
    jumpTarget,
    numPages,
    pageSizesPt,
    effectiveZoom,
    setLayout,
    setPaneWidth,
    setPdfGeometry,
    setPageSizes,
    pageSizeFor,
    zoomIn,
    zoomOut,
    toggleFit,
    gotoPage,
    stepPage,
    setVisiblePage,
    restorePageFor,
  };
});

function clampZoom(n: number): number {
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, Math.round(n * 1000) / 1000));
}
