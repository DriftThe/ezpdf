<script setup lang="ts">
import { computed } from "vue";
import type { PDFDocumentProxy } from "pdfjs-dist";
import { useReaderStore } from "../../stores/reader";
import type { Block } from "../../types/domain";
import PdfPageCanvas from "./PdfPageCanvas.vue";

/**
 * 单页卡：pdfjs canvas（PdfPageCanvas 自虚拟化）+ 块覆盖层。
 * 卡片几何 = widthPt/heightPt × zoom（来自 pdfjs 第 1 页实测；各页尺寸不同的
 * PDF 会有轻微拉伸——v1 按"各页同尺寸"假设处理）。
 * 覆盖层体系不变：块 loc 角点 → 百分比定位，原文虚线框 / 译文白底覆盖。
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

/** 原文栏：块类型 → 虚线框颜色（figure 绿、formula 紫、table 橙） */
function typeClass(type: string): string {
  switch (type) {
    case "figure":
      return "lbl-figure";
    case "formula":
      return "lbl-formula";
    case "table":
      return "lbl-table";
    case "title":
      return "lbl-title";
    default:
      return "lbl-text";
  }
}
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

      <!-- 译文：仅对有内容的块做白底覆盖（figure 等不覆盖） -->
      <template v-else>
        <div
          v-for="(r, ri) in rects"
          v-show="r.block.content"
          :key="ri"
          class="blk-cover"
          :style="{ left: r.left + '%', top: r.top + '%', width: r.width + '%', height: r.height + '%' }"
          :title="r.block.type"
        >
          <span v-if="r.block.translation" class="cover-text">{{ r.block.translation }}</span>
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

/* ---- 译文：白底覆盖 ---- */
.blk-cover {
  position: absolute;
  background: #fff;
  border: 1px solid rgba(0, 0, 0, 0.06);
  border-radius: 2px;
  overflow: hidden;
  padding: 3px 5px;
}
.cover-text {
  display: block;
  font-size: 9px;
  line-height: 1.4;
  color: #333;
  overflow: hidden;
  max-height: 100%;
}
</style>
