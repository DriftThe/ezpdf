<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from "vue";
import type { PDFDocumentProxy, PDFPageProxy } from "pdfjs-dist";

/** Single-page pdfjs canvas, self-virtualized by IntersectionObserver (renders on entry, releases the
 *  bitmap on exit → flat memory). CSS size comes from the page-card; backing store = viewport×zoom×DPR. */
const props = defineProps<{
  doc: PDFDocumentProxy | null;
  pageNumber: number;
  zoom: number;
}>();

const canvasEl = ref<HTMLCanvasElement | null>(null);
type RenderTask = ReturnType<PDFPageProxy["render"]>;

let task: RenderTask | null = null;
let seq = 0; // only the latest render may commit
let io: IntersectionObserver | null = null;
let visible = false;
let timer = 0;
let lastDoc: PDFDocumentProxy | null = null;
let lastPage = 0;
let lastZoom = 0;

function cancelTask(): void {
  if (!task) return;
  try {
    task.cancel();
  } catch {
    // cancelling a finished task is a no-op; some implementations throw — ignore
  }
  task = null;
}

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
    { rootMargin: "100% 0px" }, // one screen of buffer: pre-render neighbouring pages
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
