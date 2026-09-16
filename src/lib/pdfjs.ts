import * as pdfjsLib from "pdfjs-dist";
import workerUrl from "pdfjs-dist/build/pdf.worker.min.mjs?url";

/**
 * pdfjs 装配（阶段2 渲染栈）：本模块是 pdfjs 的唯一导入入口，负责——
 * 1. worker 一次性装配：Vite 以 ?url 产出独立资产，dev/生产打包均同源可加载；
 *    worker 加载失败时 pdfjs 自动退化为主线程渲染（仅 console 告警），功能不受阻。
 * 2. 运行时资源注入：CJK cMaps（中文不错位/不缺字的前提）、标准 14 字体、
 *    wasm 解码器（JPEG2000 等）、ICC 色彩配置。目录由 scripts/copy-pdfjs-assets.mjs
 *    从 node_modules 拷到 public/pdfjs/（pnpm dev / pnpm build 前跑）。
 */
pdfjsLib.GlobalWorkerOptions.workerSrc = workerUrl;

/** 打开 PDF 文档：自动附带全部运行时资源路径（调用方只管给文档 URL） */
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
