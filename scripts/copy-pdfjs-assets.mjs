/**
 * Copy pdfjs runtime assets into public/pdfjs/ (run before dev and build):
 * - cmaps: CJK encoding maps; missing them breaks Chinese text
 * - standard_fonts: standard 14 fonts
 * - wasm: JPEG2000 and other image decoders (wasm since v5)
 * - iccs: ICC color profiles
 * public/ is served by Vite in dev and copied into dist on build, so Tauri bundles it.
 * The output is gitignored and regenerated on the first dev/build after a reinstall.
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
