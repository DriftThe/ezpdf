import { defineStore } from "pinia";
import { computed, ref } from "vue";
import type { LayoutMode } from "../types/domain";
import { useLibraryStore } from "./library";

const MIN_ZOOM = 0.1;
const MAX_ZOOM = 4;

export const useReaderStore = defineStore("reader", () => {
  const layout = ref<LayoutMode>("ot");
  /** width = 适应窗宽（默认，不出水平滚动条）；custom = 用户手动缩放 */
  const fitMode = ref<"width" | "custom">("width");
  /** custom 模式下的缩放（pt→px 倍率，1 = 原始尺寸） */
  const zoom = ref(1);
  /** ReaderPane 上报的滚动容器宽度（px，不含滚动条） */
  const paneWidth = ref(0);
  /** 1-based 当前页 */
  const currentPage = ref(1);
  /** 原文块悬浮译文开关（关闭时原文指示框一并隐藏） */
  const hoverPreview = ref(true);
  /** 每份 PDF 记住的阅读位置 */
  const pageByPdf = ref<Record<string, number>>({});
  /** 待执行的跳页目标（由 gotoPage/切换 PDF 设置；ReaderArea 消费后清空。手动滚动不设置，避免打架） */
  const jumpTarget = ref<number | null>(null);

  /** 页数：页列表长度（绑定 JSON 无独立 meta） */
  const pageCount = computed(() => useLibraryStore().currentPdf?.pages.length ?? 0);

  /** 实际渲染缩放：适应宽度需页面几何（pt 宽度），绑定 JSON 无页面尺寸——
   *  阶段2 由 pdfjs getViewport 实测后接入；现在无几何可算，适应宽度模式退化为手动缩放 */
  const effectiveZoom = computed(() => zoom.value);

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
  /** 工具栏百分比按钮：适应宽度 ↔ 100% */
  function toggleFit(): void {
    if (fitMode.value === "width") {
      fitMode.value = "custom";
      zoom.value = 1;
    } else {
      fitMode.value = "width";
    }
  }
  /** 手动缩放前先退出适应模式，以当前实际比例为起点 */
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
  /** 阅读滚动时由 ReaderArea 上报可视页（不触发滚动，避免与手动滚动打架） */
  function setVisiblePage(p: number): void {
    if (p >= 1 && p <= (pageCount.value || 1) && p !== currentPage.value) {
      currentPage.value = p;
      rememberCurrent();
    }
  }
  /** 切换 PDF 时恢复上次阅读位置 */
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
    zoom,
    paneWidth,
    currentPage,
    hoverPreview,
    pageCount,
    jumpTarget,
    effectiveZoom,
    setLayout,
    setPaneWidth,
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
