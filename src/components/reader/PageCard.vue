<script setup lang="ts">
import { computed } from "vue";
import { useReaderStore } from "../../stores/reader";
import type { Block, PageInfo } from "../../types/domain";

/**
 * 单页占位卡（骨架期）。
 * 阶段2起：内部替换为 pdfjs canvas 渲染 + 覆盖层；块矩形按 loc 角点
 * 换算百分比定位。绑定 JSON 无页面尺寸，暂以 A4 点数（595×842pt）作占位几何，
 * 阶段2 由 pdfjs getViewport 实测替换。
 * zoom 语义：pt→px 倍率（100% = 1pt:1px），适应宽度时由 store 实时计算。
 */
const props = defineProps<{
  page: PageInfo;
  kind: "original" | "translation";
  zoom: number;
}>();

const reader = useReaderStore();
/** 占位页面几何：A4 @72dpi；阶段2 由 pdfjs 实测替换 */
const PAGE_W_PT = 595;
const PAGE_H_PT = 842;
const width = computed(() => Math.round(PAGE_W_PT * props.zoom));
const height = computed(() => Math.round(PAGE_H_PT * props.zoom));

interface Rect {
  block: Block;
  left: number;
  top: number;
  width: number;
  height: number;
}

/** loc = [x1, y1, x2, y2] 左上→右下角点 → 页面百分比矩形 */
const rects = computed<Rect[]>(() =>
  props.page.blocks.map((b) => ({
    block: b,
    left: (b.loc[0] / PAGE_W_PT) * 100,
    top: (b.loc[1] / PAGE_H_PT) * 100,
    width: ((b.loc[2] - b.loc[0]) / PAGE_W_PT) * 100,
    height: ((b.loc[3] - b.loc[1]) / PAGE_H_PT) * 100,
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
  <div class="page-wrap" :data-page-index="page.index - 1">
    <div class="page-card" :style="{ width: width + 'px', height: height + 'px' }">
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

      <!-- 译文：仅对有内容的块做白底覆盖 + 译文占位（figure 等不覆盖） -->
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
    <div class="page-num">{{ page.index }}</div>
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
