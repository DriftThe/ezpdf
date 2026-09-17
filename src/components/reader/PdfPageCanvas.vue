<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import type { PDFDocumentProxy, PDFPageProxy } from "pdfjs-dist";

/**
 * Single-page pdfjs canvas, self-virtualized: an IntersectionObserver (root=null,
 * one screen of buffer above/below) renders on entry and releases the bitmap
 * (width=0) on exit — memory stays flat for large PDFs.
 * CSS size comes from the parent page-card (canvas 100%, true per-page geometry);
 * the backing store renders at that page's viewport×zoom×DPR for sharpness. While
 * zoom changes, the old bitmap is stretched via CSS, then re-rendered after debounce.
 */
const props = defineProps<{
  doc: PDFDocumentProxy | null;
  pageNumber: number;
  zoom: number;
}>();

const canvasEl = ref<HTMLCanvasElement | null>(null);
type RenderTask = ReturnType<PDFPageProxy["render"]>;

let task: RenderTask | null = null;
let seq = 0; // render sequence: only the latest result may commit
let io: IntersectionObserver | null = null;
let visible = false;
let timer = 0;
/** Last committed bitmap identity (doc ref + page number + zoom); a hit skips re-render */
let lastDoc: PDFDocumentProxy | null = null;
let lastPage = 0;
let lastZoom = 0;

function cancelTask(): void {
  if (!task) return;
  try {
    task.cancel();
  } catch {
    // Cancelling a finished task is a no-op; some implementations may throw — ignore
  }
  task = null;
}

/** Release the bitmap (scrolled out of the render window/unmounted) */
function release(): void {
  seq++; // invalidate in-flight renders
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
    // Since v6, render requires a canvas (pdfjs fetches the context itself)
    const t = page.render({ canvas, viewport });
    task = t;
    await t.promise;
    if (mySeq !== seq) return; // superseded by a newer render/release
    lastDoc = doc;
    lastPage = myPage;
    lastZoom = myZoom;
  } catch (e) {
    // Render cancellation (interrupted by a new render/release) is not an error
    if ((e as { name?: string } | null)?.name !== "RenderingCancelledException") {
      console.warn(`[PdfPageCanvas] render page ${myPage} failed:`, e);
    }
  }
}

/** Visibility/input change → render, release, or debounced re-render */
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
    { rootMargin: "100% 0px" }, // one screen above/below: pre-render neighbouring pages
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
