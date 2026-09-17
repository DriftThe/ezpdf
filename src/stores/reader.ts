import { defineStore } from "pinia";
import { computed, ref } from "vue";
import type { LayoutMode } from "../types/domain";
import { useLibraryStore } from "./library";

const MIN_ZOOM = 0.1;
const MAX_ZOOM = 4;
const PAGE_FIT_PADDING = 32;

export const useReaderStore = defineStore("reader", () => {
  const layout = ref<LayoutMode>("ot");
  /** width = fit pane (default); custom = manual zoom (pt→px factor, 1 = original). */
  const fitMode = ref<"width" | "custom">("width");
  const zoom = ref(1);
  /** Scroll container width reported by ReaderPane (px, scrollbar excluded). */
  const paneWidth = ref(0);
  const currentPage = ref(1);
  /** Hover toggle (also hides the original-pane indicator rects). */
  const hoverPreview = ref(true);
  const pageByPdf = ref<Record<string, number>>({});
  /** Pending jump target (set by gotoPage/PDF switch, cleared by ReaderArea; manual scroll leaves it alone). */
  const jumpTarget = ref<number | null>(null);

  const numPages = ref(0);
  const pageSizePt = ref({ w: 0, h: 0 });
  /** Per-page size (pt) — overlays must divide by the page's own viewport, not page 1's. */
  const pageSizesPt = ref<Array<{ w: number; h: number }>>([]);

  function setPdfGeometry(pages: number, w: number, h: number): void {
    numPages.value = pages;
    pageSizePt.value = { w, h };
  }

  function setPageSizes(sizes: Array<{ w: number; h: number }>): void {
    pageSizesPt.value = sizes;
  }

  function pageSizeFor(n: number): { w: number; h: number } {
    return pageSizesPt.value[n - 1] ?? pageSizePt.value;
  }

  const pageCount = computed(() =>
    numPages.value > 0 ? numPages.value : (useLibraryStore().currentPdf?.pages.length ?? 0),
  );
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
  function toggleFit(): void {
    if (fitMode.value === "width") {
      fitMode.value = "custom";
      zoom.value = 1;
    } else {
      fitMode.value = "width";
    }
  }
  /** Start manual zoom from the current effective zoom. */
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
  /** Visible page reported while scrolling (does not scroll back). */
  function setVisiblePage(p: number): void {
    if (p >= 1 && p <= (pageCount.value || 1) && p !== currentPage.value) {
      currentPage.value = p;
      rememberCurrent();
    }
  }
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
