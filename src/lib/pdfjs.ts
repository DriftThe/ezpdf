import * as pdfjsLib from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";

/** Only pdfjs entry: worker + runtime resources (cMaps/fonts/wasm/ICC); missing them breaks CJK. */
pdfjsLib.GlobalWorkerOptions.workerSrc = workerUrl;

export function openPdf(url: string) {
  return pdfjsLib.getDocument({
    url,
    cMapUrl: "/pdfjs/cmaps/",
    cMapPacked: true,
    standardFontDataUrl: "/pdfjs/standard_fonts/",
    wasmUrl: "/pdfjs/wasm/",
    iccUrl: "/pdfjs/iccs/",
  });
}
