<script setup lang="ts">
import { computed } from "vue";
import type { Directive } from "vue";
import type { PDFDocumentProxy } from "pdfjs-dist";
import { useReaderStore } from "../../stores/reader";
import type { Block } from "../../types/domain";
import { isOverlayType } from "../../lib/blocks";
import { renderRichText } from "../../lib/richText";
import PdfPageCanvas from "./PdfPageCanvas.vue";

/**
 * 单页卡：pdfjs canvas（PdfPageCanvas 自虚拟化）+ 块覆盖层。
 * 卡片几何 = 每页真实 widthPt/heightPt × zoom（逐页实测，reader.pageSizeFor；
 * 旧版曾按第 1 页尺寸统一假设——页尺寸不一的扫描版 PDF 会整体错位，已改）。
 * 覆盖层体系（不改原 PDF 排版，全部绝对定位）：
 * - 原文栏：全部块虚线框标注（悬浮预览开关控制）；
 * - 译文栏：仅覆盖渲染类型（lib/blocks.ts OVERLAY_TYPES = 送翻 + formula）白底覆盖框，
 *   框内文字 =
 *   translation ?? content（bypass 期 translation 全 null → 显示原文 content），
 *   字号经 v-fit 自适应：二分找"塞得下"的最大字号，填满且不溢出。
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

/** loc = [x1, y1, x2, y2] 左上→右下角点 → 页面百分比矩形 */
const rects = computed<Rect[]>(() =>
  props.blocks.map((b) => ({
    block: b,
    left: (b.loc[0] / props.widthPt) * 100,
    top: (b.loc[1] / props.heightPt) * 100,
    width: ((b.loc[2] - b.loc[0]) / props.widthPt) * 100,
    height: ((b.loc[3] - b.loc[1]) / props.heightPt) * 100,
  })),
);

/** 原文栏：块类型 → 虚线框颜色（figure/image 绿、formula 紫、table 橙、标题加底边） */
function typeClass(type: string): string {
  switch (type) {
    case "figure":
    case "image":
      return "lbl-figure";
    case "formula":
      return "lbl-formula";
    case "table":
      return "lbl-table";
    case "title":
    case "paragraph_title":
    case "doc_title":
      return "lbl-title";
    default:
      return "lbl-text";
  }
}

interface CoverRect extends Rect {
  /** 内容 HTML：正文转义 + 公式 KaTeX（lib/richText.ts） */
  html: string;
}

/** 译文栏覆盖块：仅覆盖渲染类型且文本非空；内容渲染成富文本（figure 空串等不渲染白框） */
const coverRects = computed<CoverRect[]>(() =>
  rects.value
    .filter(
      (r) => isOverlayType(r.block.type) && (r.block.translation ?? r.block.content).trim(),
    )
    .map((r) => ({ ...r, html: renderRichText(r.block.translation ?? r.block.content) })),
);

// ---- v-fit：白底框字号自适应（框尺寸 × 文字量 → 填满、不溢出） ----
// 性能约束（2026-09-11 实测）：大书（200 页 × 十余块）缩放时全部覆盖框同时变尺寸，
// 若逐框立即二分量算（强制同步布局），单次 zoom 阻塞主线程 ~3s。故只对视口附近的框
// 量算：离屏框仅标脏（WeakSet，零布局），进入视口（IntersectionObserver，上下扩一屏）
// 时才真正适配；updated / ResizeObserver 对离屏框同样退化为标脏。

const roMap = new WeakMap<HTMLElement, ResizeObserver>();
const visibleBoxes = new Set<HTMLElement>();
const dirtyBoxes = new WeakSet<HTMLElement>();
const pendingFits = new Set<HTMLElement>();
let flushRaf = 0;

// KaTeX 字体异步加载完成 = 字宽/行高变化：对当前可见框重适配一次（数量小，代价可忽略）
if (typeof document !== "undefined" && "fonts" in document) {
  document.fonts.addEventListener("loadingdone", () => {
    for (const box of visibleBoxes) fitCoverText(box);
  });
}

function fitCoverText(box: HTMLElement): void {
  const span = box.querySelector<HTMLElement>(".cover-text");
  if (!span) return;
  const cs = getComputedStyle(box);
  const inner =
    box.clientHeight - (parseFloat(cs.paddingTop) + parseFloat(cs.paddingBottom));
  if (inner <= 0) return;
  const MIN = 5;
  const fits = (size: number): boolean => {
    span.style.fontSize = `${size}px`;
    return span.scrollHeight <= inner;
  };
  if (!fits(MIN)) return; // 最小号仍溢出：保持 MIN，overflow:hidden 兜底截断
  let lo = MIN;
  let hi = Math.max(MIN, inner); // 单行封顶 = 框内容高（再大必然纵向溢出）
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (fits(mid)) lo = mid;
    else hi = mid - 1;
  }
  // 显式收敛到最终值：循环里最后一次探测可能是"失败"的更大字号，
  // 不重设的话元素停留在溢出字号上（部分框溢出的根因）
  span.style.fontSize = `${lo}px`;
}

/** 请求适配：离屏框只标脏等 IO；视口内框入批，帧末统一量算（updated/RO 同帧去重） */
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
        fitCoverText(box); // 初始/滚入上报：绘制前完成适配，避免默认字号闪现
      }
    }
  },
  { rootMargin: "50% 0px" }, // 进入前约半屏即适配（画布虚拟化仍是上下各一屏）
);

const vFit: Directive<HTMLElement> = {
  mounted(box) {
    dirtyBoxes.add(box); // 不立即量算：等 IO 初报，离屏框不付布局代价
    fitIO.observe(box);
    const ro = new ResizeObserver(() => requestFit(box)); // zoom/窗口变化 → 重适配
    ro.observe(box);
    roMap.set(box, ro);
  },
  updated(box) {
    requestFit(box); // 文本/几何更新后重新适配
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
      <!-- pdfjs canvas：原/译两栏同一渲染，差异全在覆盖层 -->
      <PdfPageCanvas :doc="doc" :page-number="pageNumber" :zoom="zoom" />

      <!-- 原文：OCR 块虚线标注（悬浮预览关闭时一并隐藏，悬浮目标随之消失） -->
      <template v-if="kind === 'original' && reader.hoverPreview">
        <div
          v-for="(r, ri) in rects"
          :key="ri"
          class="blk-line"
          :class="typeClass(r.block.type)"
          :style="{ left: r.left + '%', top: r.top + '%', width: r.width + '%', height: r.height + '%' }"
          :title="`[${r.block.type}]` + (r.block.translation ? ` ${r.block.translation}` : '')"
        />
      </template>

      <!-- 译文：仅覆盖渲染类型白底覆盖（未覆盖类型原 PDF 像素直出）；
           框内 translation ?? content：bypass 期全 null → 显示原文 content -->
      <template v-if="kind === 'translation'">
        <div
          v-for="(r, ri) in coverRects"
          :key="ri"
          v-fit
          class="blk-cover"
          :class="{ 'cover-formula': r.block.type === 'formula' }"
          :style="{ left: r.left + '%', top: r.top + '%', width: r.width + '%', height: r.height + '%' }"
          :title="r.block.type"
        >
          <span class="cover-text" v-html="r.html"></span>
        </div>
      </template>
    </div>
    <div class="page-num">{{ pageNumber }}</div>
  </div>
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

/* ---- 原文：虚线块（页面恒白底，用主题无关的标注色） ---- */
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

/* ---- 译文：白底覆盖（不送翻类型零覆盖） ---- */
.blk-cover {
  position: absolute;
  background: #fff;
  border: 1px solid rgba(0, 0, 0, 0.06);
  border-radius: 2px;
  overflow: hidden;
  padding: 2px 4px;
}
/* 公式：KaTeX 垂直居中（用户 2026-09-14）；KaTeX display 公式自带 1em 上下
   margin，在 ~16pt 高的公式框里会把内容顶到贴顶、还逼 v-fit 选极小字号——收窄 */
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
  /* 字号由 v-fit 指令按框尺寸×文字量动态设定 */
  overflow-wrap: anywhere; /* 长词/URL 也能折行，宽度不溢出 */
}
</style>
