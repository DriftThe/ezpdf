<script setup lang="ts">
import { computed } from "vue";
import { useReaderStore } from "../../stores/reader";
import type { Block, PageInfo } from "../../types/domain";

/**
 * 单页占位卡（骨架期）。
 * 阶段2起：内部替换为 pdfjs canvas 渲染 + 覆盖层；块矩形坐标已按
 * bboxPt/页面尺寸 百分比定位，与未来渲染几何完全一致。
 * zoom 语义：pt→px 倍率（100% = 1pt:1px），适应宽度时由 store 实时计算。
 */
const props = defineProps<{
  page: PageInfo;
  kind: "original" | "translation";
  zoom: number;
}>();

const reader = useReaderStore();
const width = computed(() => Math.round(props.page.widthPt * props.zoom));
const height = computed(() => Math.round(props.page.heightPt * props.zoom));

interface Rect {
  block: Block;
  left: number;
  top: number;
  width: number;
  height: number;
}

const rects = computed<Rect[]>(() =>
  props.page.blocks.map((b) => ({
    block: b,
    left: (b.bboxPt[0] / props.page.widthPt) * 100,
    top: (b.bboxPt[1] / props.page.heightPt) * 100,
    width: (b.bboxPt[2] / props.page.widthPt) * 100,
    height: (b.bboxPt[3] / props.page.heightPt) * 100,
  })),
);

const statusChip = computed(() => {
  switch (props.page.status) {
    case "translating":
      return { text: "翻译中", cls: "busy" };
    case "ocr_queued":
    case "ocr_done":
      return { text: "解析中", cls: "busy" };
    case "failed":
      return { text: "失败", cls: "fail" };
    default:
      return null;
  }
});

/** 原文栏：块类型 → 虚线框颜色（figure 绿、formula 紫、table 橙） */
function labelClass(label: string): string {
  switch (label) {
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
  <div class="page-wrap" :data-page-index="page.index">
    <div class="page-card" :style="{ width: width + 'px', height: height + 'px' }">
      <!-- 原文：OCR 块虚线标注（悬浮预览关闭时一并隐藏，悬浮目标随之消失） -->
      <template v-if="kind === 'original' && reader.hoverPreview">
        <div
          v-for="r in rects"
          :key="r.block.id"
          class="blk-line"
          :class="labelClass(String(r.block.label))"
          :style="{ left: r.left + '%', top: r.top + '%', width: r.width + '%', height: r.height + '%' }"
          :title="`[${r.block.label}]` + (r.block.translation ? ` ${r.block.translation}` : '')"
        />
      </template>

      <!-- 译文：仅对有内容的块做白底覆盖 + 译文占位（figure 等不覆盖） -->
      <template v-else>
        <div
          v-for="r in rects"
          v-show="r.block.source"
          :key="r.block.id"
          class="blk-cover"
          :style="{ left: r.left + '%', top: r.top + '%', width: r.width + '%', height: r.height + '%' }"
          :title="r.block.label"
        >
          <span v-if="r.block.translation" class="cover-text">{{ r.block.translation }}</span>
        </div>
      </template>

      <span v-if="statusChip" class="page-chip" :class="statusChip.cls">{{ statusChip.text }}</span>
    </div>
    <div class="page-num">{{ page.index + 1 }}</div>
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

/* ---- 状态角标 ---- */
.page-chip {
  position: absolute;
  top: 6px;
  right: 6px;
  font-size: 10px;
  padding: 1px 7px;
  border-radius: 9px;
  color: #fff;
  background: var(--pending);
}
.page-chip.busy {
  background: var(--page-annot);
  animation: chip-pulse 1.4s ease-in-out infinite;
}
.page-chip.fail {
  background: var(--err);
}
@keyframes chip-pulse {
  50% {
    opacity: 0.55;
  }
}
</style>
