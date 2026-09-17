/**
 * Copy pdfjs runtime assets into public/pdfjs/ (gitignored, regenerated before dev/build):
 * cmaps (CJK encoding maps), standard_fonts, wasm (JPEG2000/image decoders, v5+), iccs (color profiles).
 * public/ is served by Vite in dev and bundled into dist on build.
 */
import { cpSync, mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const src = path.join(root, "node_modules", "pdfjs-dist");
const out = path.join(root, "public", "pdfjs");

mkdirSync(out, { recursive: true });
for (const dir of ["cmaps", "standard_fonts", "wasm", "iccs"]) {
  cpSync(path.join(src, dir), path.join(out, dir), { recursive: true });
}
console.log("[copy-pdfjs-assets] cmaps/standard_fonts/wasm/iccs -> public/pdfjs/");
