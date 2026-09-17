import type { PDFDocumentProxy } from "pdfjs-dist";

/** OCR input render scale; Rust converts px→pt (pt = px/scale) when writing back. */
export const RENDER_SCALE = 2.0;

/** Offscreen-render a page → base64 PNG (no data: prefix, ready as image_b64).
 *  Works regardless of pane visibility (virtualized or background pages) via the cached
 *  doc; the backing store is freed right after, so batch memory stays flat. */
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
    // v6 render requires canvas (as in PdfPageCanvas); pdfjs fetches the context itself
    await page.render({ canvas, viewport }).promise;
    const dataUrl = canvas.toDataURL("image/png");
    const comma = dataUrl.indexOf(",");
    return comma >= 0 ? dataUrl.slice(comma + 1) : dataUrl;
  } finally {
    canvas.width = 0; // free the bitmap immediately
    canvas.height = 0;
  }
}
