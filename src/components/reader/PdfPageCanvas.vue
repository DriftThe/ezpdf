<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import type { PDFDocumentProxy, PDFPageProxy } from "pdfjs-dist";

/**
 * 单页 pdfjs canvas，自虚拟化：IntersectionObserver 监听（root=null 视口，上下各扩
 * 一屏缓冲），进入才渲染、滚出即释放位图（width=0）——大 PDF 内存恒定。
 * CSS 尺寸由父级 page-card 决定（canvas width/height 100%，逐页真实几何），
 * backing store 按本页 viewport×zoom×DPR 渲染保证清晰度；zoom 变化先由旧位图
 * CSS 拉伸顶住，防抖后重渲。
 */
const props = defineProps<{
  doc: PDFDocumentProxy | null;
  pageNumber: number;
  zoom: number;
}>();

const canvasEl = ref<HTMLCanvasElement | null>(null);
type RenderTask = ReturnType<PDFPageProxy["render"]>;

let task: RenderTask | null = null;
let seq = 0; // 渲染序号：只有最新一次的结果允许落盘
let io: IntersectionObserver | null = null;
let visible = false;
let timer = 0;
/** 上次成功落盘的位图标识（doc 引用 + 页号 + zoom），命中则跳过重渲 */
let lastDoc: PDFDocumentProxy | null = null;
let lastPage = 0;
let lastZoom = 0;

function cancelTask(): void {
  if (!task) return;
  try {
    task.cancel();
  } catch {
    // 已完成的 task 取消是 no-op，个别实现可能抛错——忽略
  }
  task = null;
}

/** 释放位图（滚出渲染窗口/卸载） */
function release(): void {
  seq++; // 作废在途渲染
  cancelTask();
  const canvas = canvasEl.value;
  if (canvas && canvas.width > 0) {
    canvas.width = 0;
    canvas.height = 0;
  }
  lastDoc = null;
  lastPage = 0;
  lastZoom = 0;
}

async function render(): Promise<void> {
  const doc = props.doc;
  const canvas = canvasEl.value;
  if (!doc || !canvas) return;
  const mySeq = ++seq;
  cancelTask();
  const myPage = props.pageNumber;
  const myZoom = props.zoom;
  try {
    const page = await doc.getPage(myPage);
    if (mySeq !== seq || !canvasEl.value) return;
    const dpr = window.devicePixelRatio || 1;
    const viewport = page.getViewport({ scale: myZoom * dpr });
    canvas.width = Math.max(1, Math.floor(viewport.width));
    canvas.height = Math.max(1, Math.floor(viewport.height));
    // v6 起 render 必传 canvas（context 由 pdfjs 自取）
    const t = page.render({ canvas, viewport });
    task = t;
    await t.promise;
    if (mySeq !== seq) return; // 已被更新的渲染/释放作废
    lastDoc = doc;
    lastPage = myPage;
    lastZoom = myZoom;
  } catch (e) {
    // 渲染取消（新渲染/释放打断）不是错误
    if ((e as { name?: string } | null)?.name !== "RenderingCancelledException") {
      console.warn(`[PdfPageCanvas] render page ${myPage} failed:`, e);
    }
  }
}

/** 可见性/入参变化 → 决定渲染、释放或防抖重渲 */
function sync(immediate = false): void {
  if (!visible || !props.doc) {
    release();
    return;
  }
  if (lastDoc === props.doc && lastPage === props.pageNumber && lastZoom === props.zoom) return;
  window.clearTimeout(timer);
  if (immediate) void render();
  else timer = window.setTimeout(() => void render(), 120);
}

watch([() => props.doc, () => props.pageNumber, () => props.zoom], () => sync());

onMounted(() => {
  io = new IntersectionObserver(
    (entries) => {
      visible = entries.some((e) => e.isIntersecting);
      if (visible) sync(true);
      else release();
    },
    { rootMargin: "100% 0px" }, // 上下各扩一屏：邻近页预渲染
  );
  if (canvasEl.value) io.observe(canvasEl.value);
});

onBeforeUnmount(() => {
  window.clearTimeout(timer);
  io?.disconnect();
  io = null;
  release();
});
</script>

<template>
  <canvas ref="canvasEl" class="pdf-canvas" />
</template>

<style scoped>
.pdf-canvas {
  display: block;
  width: 100%;
  height: 100%;
  background: #fff;
}
</style>
