import type { PDFDocumentProxy } from "pdfjs-dist";

/** OCR 输入渲染倍率（前后端约定常量；px→pt 换算 pt = px/scale 由 Rust 写回时做） */
export const RENDER_SCALE = 2.0;

/**
 * 离屏渲染一页 → PNG base64（无 data: 前缀，直接作 /ocr/pages 的 image_b64）。
 * 调度桥的渲染来源不依赖两栏可见性：虚拟化已释放位图的页、后台书的页都能出图
 * （复用 usePdfDoc 按 id 缓存的 doc）；渲染完立即释放 backing store，批量内存恒定。
 */
export async function renderPageToDataUrl(
  doc: PDFDocumentProxy,
  pageNumber: number,
): Promise<string> {
  const page = await doc.getPage(pageNumber);
  const viewport = page.getViewport({ scale: RENDER_SCALE });
  const canvas = document.createElement("canvas");
  canvas.width = Math.max(1, Math.floor(viewport.width));
  canvas.height = Math.max(1, Math.floor(viewport.height));
  try {
    // v6 render 必传 canvas（与 PdfPageCanvas 同法；context 由 pdfjs 自取）
    await page.render({ canvas, viewport }).promise;
    const dataUrl = canvas.toDataURL("image/png");
    const comma = dataUrl.indexOf(",");
    return comma >= 0 ? dataUrl.slice(comma + 1) : dataUrl;
  } finally {
    canvas.width = 0; // 位图即刻释放
    canvas.height = 0;
  }
}
