<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import { fitFontSize } from "../../composables/fitFont";
import type { TableGrid } from "../../types/domain";
import { renderRichText } from "../../lib/richText";
import { parseTableMatrix } from "../../lib/table";

/**
 * Table cover: redraws an OCR table as a web table in the translation pane.
 * `grid` + `translation` (2-D matrix JSON) come from the bound JSON; colspan merges via CSS grid;
 * text goes through renderRichText (inline `\(...\)` → KaTeX).
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
/** Doesn't fit → whole cover transparent (truncating a table would lose content, unlike text/formula v-fit) */
const fitted = ref(true);

const MIN = 5;
const MAX = 20;

function fit(): void {
  const box = el.value;
  if (!box) return;
  const size = fitFontSize(
    (px) => {
      box.style.setProperty("--tbl-font", `${px}px`);
      return box.scrollHeight <= box.clientHeight + 1 && box.scrollWidth <= box.clientWidth + 1;
    },
    MIN,
    MAX,
  );
  if (size === MIN && !(box.scrollHeight <= box.clientHeight + 1)) {
    fitted.value = false;
    return;
  }
  box.style.setProperty("--tbl-font", `${size}px`);
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

watch([() => props.translation, () => props.grid], scheduleFit);
</script>

<template>
  <div
    ref="el"
    class="tbl-cover cover-box"
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
/* Base styles in main.css .cover-box; size comes from the loc percentage */
.tbl-cover {
  padding: 1px;
  --tbl-font: 10px;
}
/* Doesn't fit: whole cover transparent → original pixels show through (truncating would lose cells) */
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
/* Bold first row: most OCR tables have a header there (visual hint only, no semantic judgement) */
.tbl td.cell-head {
  font-weight: 600;
  background: rgba(0, 0, 0, 0.03);
}
.tbl-text :deep(.katex) {
  font-size: 1em;
}
</style>
