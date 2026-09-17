import * as pdfjsLib from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";

/**
 * The only pdfjs import entry. Sets up the worker once (Vite ?url asset; falls back to
 * main-thread rendering on failure) and injects runtime resources — CJK cMaps, standard
 * fonts, wasm decoders, ICC — copied into public/pdfjs/ by copy-pdfjs-assets.mjs.
 */
pdfjsLib.GlobalWorkerOptions.workerSrc = workerUrl;

/** Open a PDF with all runtime resource paths attached (caller just passes the URL). */
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
