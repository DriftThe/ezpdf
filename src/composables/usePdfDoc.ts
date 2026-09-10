import { convertFileSrc } from "@tauri-apps/api/core";
import { openPdf } from "../lib/pdfjs";
import type { PDFDocumentProxy } from "pdfjs-dist";

/**
 * pdfjs 文档加载与缓存：键 = PDF 稳定 id（同一本书的 doc 两栏复用）。
 * 释放由调用方（ReaderArea）在切书/卸载时驱动 destroyPdfDoc；
 * 加载失败自动清缓存，允许重试。
 */
const cache = new Map<string, Promise<PDFDocumentProxy>>();

export function loadPdfDoc(id: string, pdfPath: string): Promise<PDFDocumentProxy> {
  const cached = cache.get(id);
  if (cached) return cached;
  // PDF 二进制不走 invoke（既定决策）：convertFileSrc 把绝对路径映射为
  // asset:// URL，由 Rust 侧 asset protocol 按仓库作用域放行后流式下发；
  // openPdf 附带 CJK cMaps/标准字体/wasm/ICC 资源路径
  const task = openPdf(convertFileSrc(pdfPath)).promise;
  cache.set(id, task);
  task.catch(() => cache.delete(id));
  return task;
}

/** 释放文档：v6 起 PDFDocumentProxy 无 destroy，走 loadingTask.destroy()
 *  （连带中止未完成的加载请求，竞态场景更干净；加载本身已失败则无事可做） */
export async function destroyPdfDoc(id: string): Promise<void> {
  const task = cache.get(id);
  cache.delete(id);
  if (!task) return;
  try {
    await (await task).loadingTask.destroy();
  } catch {
    // destroy 失败不影响主流程（worker 已崩等场景）
  }
}
