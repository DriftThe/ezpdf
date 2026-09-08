import { defineStore } from "pinia";
import { computed, ref } from "vue";
import type { LayoutMode } from "../types/domain";
import { useLibraryStore } from "./library";

const MIN_ZOOM = 0.4;
const MAX_ZOOM = 3;

export const useReaderStore = defineStore("reader", () => {
  const layout = ref<LayoutMode>("ot");
  const zoom = ref(1);
  /** 1-based 当前页 */
  const currentPage = ref(1);
  /** 原文块悬浮译文开关 */
  const hoverPreview = ref(true);
  /** 每本书记住的阅读位置 */
  const pageByBook = ref<Record<string, number>>({});
  /** 待执行的跳页目标（由 gotoPage/切书设置；ReaderArea 消费后清空。手动滚动不设置，避免打架） */
  const jumpTarget = ref<number | null>(null);

  const pageCount = computed(() => useLibraryStore().currentBook?.meta.pageCount ?? 0);

  function setLayout(l: LayoutMode): void {
    layout.value = l;
  }
  function zoomIn(): void {
    zoom.value = Math.min(MAX_ZOOM, round3(zoom.value * 1.15));
  }
  function zoomOut(): void {
    zoom.value = Math.max(MIN_ZOOM, round3(zoom.value / 1.15));
  }
  function resetZoom(): void {
    zoom.value = 1;
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
  /** 阅读滚动时由 ReaderArea 上报可视页（不触发滚动，避免与手动滚动打架） */
  function setVisiblePage(p: number): void {
    if (p >= 1 && p <= (pageCount.value || 1) && p !== currentPage.value) {
      currentPage.value = p;
      rememberCurrent();
    }
  }
  /** 切换书籍时恢复上次阅读位置 */
  function restorePageFor(bookId: string): void {
    currentPage.value = pageByBook.value[bookId] ?? 1;
    jumpTarget.value = currentPage.value;
  }
  function rememberCurrent(): void {
    const lib = useLibraryStore();
    if (lib.currentBookId) {
      pageByBook.value[lib.currentBookId] = currentPage.value;
    }
  }

  return {
    layout,
    zoom,
    currentPage,
    hoverPreview,
    pageCount,
    jumpTarget,
    setLayout,
    zoomIn,
    zoomOut,
    resetZoom,
    gotoPage,
    stepPage,
    setVisiblePage,
    restorePageFor,
  };
});

function round3(n: number): number {
  return Math.round(n * 1000) / 1000;
}
