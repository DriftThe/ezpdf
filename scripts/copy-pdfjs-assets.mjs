/**
 * 拷贝 pdfjs 运行时资源到 public/pdfjs/（dev 与 build 前各跑一次，见 package.json scripts）：
 * - cmaps：CJK 字符编码映射——缺失时中文错位/缺字（本脚本存在的根因）
 * - standard_fonts：标准 14 字体数据
 * - wasm：JPEG2000 等图像解码器（v5+ 起 wasm 化）
 * - iccs：ICC 色彩配置
 * public/ 在 dev 被 Vite 直接服务、build 时拷进 dist → Tauri 打包自动带上。
 * 产物不入库（.gitignore），node_modules 重装后首次 dev/build 自动补齐。
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
