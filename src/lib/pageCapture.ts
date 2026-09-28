import type { PDFDocumentProxy } from "pdfjs-dist";

/** OCR input render scale; Rust converts px→pt (pt = px/scale) when writing back. */
export const RENDER_SCALE = 2.0;

/** Offscreen render → base64 PNG (no prefix); works for virtualized/background pages. */
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
    // v6 render requires canvas; pdfjs fetches the context itself
    await page.render({ canvas, viewport }).promise;
    const dataUrl = canvas.toDataURL("image/png");
    const comma = dataUrl.indexOf(",");
    return comma >= 0 ? dataUrl.slice(comma + 1) : dataUrl;
  } finally {
    canvas.width = 0;
    canvas.height = 0;
  }
}

/**
 * The page's size in pt at scale 1 — the space `Block.loc` is written in and the covers divide by.
 *
 * With the Mistral-shaped service the render happens server-side, so Rust places boxes by the ratio
 * between the service's raster and this size; it is the same viewport the reader itself measures
 * (stores/reader.ts pageSizeFor), which is what keeps the two panes aligned.
 */
export async function pageSizePt(
  doc: PDFDocumentProxy,
  pageNumber: number,
): Promise<[number, number]> {
  const page = await doc.getPage(pageNumber);
  const viewport = page.getViewport({ scale: 1 });
  return [viewport.width, viewport.height];
}
