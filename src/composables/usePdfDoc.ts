import { convertFileSrc } from "@tauri-apps/api/core";
import { openPdf } from "../lib/pdfjs";
import type { PDFDocumentProxy } from "pdfjs-dist";

/**
 * pdfjs 文档加载与缓存：键 = PDF 稳定 id（同一本书的 doc 两栏复用 + 调度桥离屏
 * 渲染复用）。LRU 上限 2 本：焦点书（pin，ReaderArea 正在用）不参与淘汰；
 * 后台解析书先到先占、LRU 淘汰。释放一律走 loadingTask.destroy()
 * （v6 起 PDFDocumentProxy 无 destroy）；加载失败自动清缓存，允许重试。
 */
const MAX_DOCS = 2;
/** insertion-ordered Map 当 LRU 用（touch = 删除重插，队尾最新） */
const cache = new Map<string, Promise<PDFDocumentProxy>>();
/** 焦点书集合——永不淘汰 */
const pinnedIds = new Set<string>();

function touch(id: string): void {
  const task = cache.get(id);
  if (!task) return;
  cache.delete(id);
  cache.set(id, task);
}

/** 超容淘汰：从最旧开始逐个销毁未 pin 的 doc */
function evictOverflow(): void {
  for (const id of cache.keys()) {
    if (cache.size <= MAX_DOCS) break;
    if (pinnedIds.has(id)) continue;
    void destroyPdfDoc(id);
  }
}

export function loadPdfDoc(id: string, pdfPath: string, pin = false): Promise<PDFDocumentProxy> {
  if (pin) pinnedIds.add(id);
  const cached = cache.get(id);
  if (cached) {
    touch(id);
    evictOverflow();
    return cached;
  }
  // PDF 二进制不走 invoke（既定决策）：convertFileSrc 把绝对路径映射为
  // asset:// URL，由 Rust 侧 asset protocol 按仓库作用域放行后流式下发；
  // openPdf 附带 CJK cMaps/标准字体/wasm/ICC 资源路径
  const task = openPdf(convertFileSrc(pdfPath)).promise;
  cache.set(id, task);
  task.catch(() => {
    cache.delete(id);
    pinnedIds.delete(id);
  });
  evictOverflow();
  return task;
}

/** 释放文档：v6 起 PDFDocumentProxy 无 destroy，走 loadingTask.destroy()
 *  （连带中止未完成的加载请求，竞态场景更干净；加载本身已失败则无事可做） */
export async function destroyPdfDoc(id: string): Promise<void> {
  pinnedIds.delete(id);
  const task = cache.get(id);
  cache.delete(id);
  if (!task) return;
  try {
    await (await task).loadingTask.destroy();
  } catch {
    // destroy 失败不影响主流程（worker 已崩等场景）
  }
}
