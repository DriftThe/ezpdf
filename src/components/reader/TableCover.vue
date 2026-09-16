<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import type { TableGrid } from "../../types/domain";
import { renderRichText } from "../../lib/richText";
import { parseTableMatrix } from "../../lib/table";

/**
 * 表格覆盖框（用户 2026-09-16）：译文栏里用网页表格重画 OCR 表格。
 *
 * - 数据来自绑定 JSON：`grid`（Rust 解析标记流后落盘）+ `translation`（二维矩阵 JSON 文本）；
 * - 自适应：二分字号塞进 loc 框，最小字号仍溢出 → 整框透明（原 PDF 像素直出，绝不比现在差）；
 * - 合并：`colspan`（`<lcel>`）用 CSS 网格跨列；纵向合并 v1 由 Rust 解析成空格子；
 * - 单元格文本走 renderRichText（`\(...\)` 行内公式渲染成 KaTeX）。
 */
const props = defineProps<{
  grid: TableGrid;
  translation: string | null;
  style: Record<string, string>;
}>();

const matrix = computed(() => parseTableMatrix(props.translation, props.grid));

interface DisplayCell {
  html: string;
  colspan: number;
}

const rows = computed<DisplayCell[][]>(() =>
  props.grid.rows.map((row, i) =>
    row.cells.map((cell, j) => {
      const translated = matrix.value?.[i]?.[j];
      const text = typeof translated === "string" ? translated : cell.text;
      return { html: renderRichText(text), colspan: Math.max(1, cell.colspan) };
    }),
  ),
);

const el = ref<HTMLElement | null>(null);
/** 塞不下 → 整框透明（text/公式那条路径由 v-fit 兜底截断，表格截断会丢内容，所以直接不覆盖） */
const fitted = ref(true);

const MIN = 5;
const MAX = 20;

function measureFits(box: HTMLElement, px: number): boolean {
  box.style.setProperty("--tbl-font", `${px}px`);
  return box.scrollHeight <= box.clientHeight + 1 && box.scrollWidth <= box.clientWidth + 1;
}

function fit(): void {
  const box = el.value;
  if (!box) return;
  if (!measureFits(box, MIN)) {
    fitted.value = false;
    return;
  }
  let lo = MIN;
  let hi = MAX;
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    if (measureFits(box, mid)) lo = mid;
    else hi = mid - 1;
  }
  box.style.setProperty("--tbl-font", `${lo}px`); // 收敛到最终值（最后一次探测可能是溢出字号）
  fitted.value = true;
}

let raf = 0;
function scheduleFit(): void {
  if (raf) return;
  raf = requestAnimationFrame(() => {
    raf = 0;
    fit();
  });
}

let ro: ResizeObserver | null = null;
const onFontsLoaded = (): void => scheduleFit();

onMounted(() => {
  ro = new ResizeObserver(scheduleFit);
  if (el.value) ro.observe(el.value);
  scheduleFit();
  if (typeof document !== "undefined" && "fonts" in document) {
    document.fonts.addEventListener("loadingdone", onFontsLoaded);
  }
});

onBeforeUnmount(() => {
  if (raf) cancelAnimationFrame(raf);
  ro?.disconnect();
  ro = null;
  if (typeof document !== "undefined" && "fonts" in document) {
    document.fonts.removeEventListener("loadingdone", onFontsLoaded);
  }
});

// 译文/网格变化（翻译完成、清除解析状态）→ 重新适配
watch([() => props.translation, () => props.grid], scheduleFit);
</script>

<template>
  <div
    ref="el"
    class="tbl-cover"
    :class="{ 'is-unfit': !fitted }"
    :style="{ ...style, '--tbl-cols': grid.cols }"
    :title="`table ${grid.cols}×${grid.rows.length}`"
  >
    <table class="tbl">
      <tbody>
        <tr v-for="(row, ri) in rows" :key="ri">
          <td
            v-for="(cell, ci) in row"
            :key="ci"
            :colspan="cell.colspan"
            :class="{ 'cell-head': ri === 0 }"
          >
            <span class="tbl-text" v-html="cell.html"></span>
          </td>
        </tr>
      </tbody>
    </table>
  </div>
</template>

<style scoped>
/* 白底覆盖框：与 PageCard 的 .blk-cover 同视觉，但内容是一张表格（尺寸由 loc 百分比定） */
.tbl-cover {
  position: absolute;
  background: #fff;
  border: 1px solid rgba(0, 0, 0, 0.06);
  border-radius: 2px;
  overflow: hidden;
  padding: 1px;
  --tbl-font: 10px;
}
/* 塞不下：整框透明 → 原 PDF 像素直出（不拦鼠标：原文栏的悬停块才是交互面） */
.tbl-cover.is-unfit {
  opacity: 0;
  pointer-events: none;
}
.tbl {
  width: 100%;
  height: 100%;
  border-collapse: collapse;
  table-layout: fixed;
  font-size: var(--tbl-font);
  line-height: 1.2;
  color: #222;
}
.tbl td {
  border: 0.5px solid rgba(0, 0, 0, 0.45);
  padding: 1px 2px;
  vertical-align: middle;
  overflow-wrap: anywhere;
  word-break: break-word;
}
/* 表头首行加粗（OCR 表格首行绝大多数是表头；不做语义判断，只是视觉提示） */
.tbl td.cell-head {
  font-weight: 600;
  background: rgba(0, 0, 0, 0.03);
}
.tbl-text :deep(.katex) {
  font-size: 1em;
}
</style>
