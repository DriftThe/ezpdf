import { convertFileSrc } from "@tauri-apps/api/core";
import { openPdf } from "../lib/pdfjs";
import type { PDFDocumentProxy } from "pdfjs-dist";

/** LRU ≤2 with the focused book pinned; destroy via loadingTask.destroy() (PDFDocumentProxy.destroy() is gone in v6). */
const MAX_DOCS = 2;
/** Insertion-ordered Map as an LRU (touch = delete + reinsert, newest last). */
const cache = new Map<string, Promise<PDFDocumentProxy>>();
const pinnedIds = new Set<string>();

function touch(id: string): void {
  const task = cache.get(id);
  if (!task) return;
  cache.delete(id);
  cache.set(id, task);
}

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
  // bytes never travel through invoke: convertFileSrc → asset:// streamed by Rust's asset scope
  const task = openPdf(convertFileSrc(pdfPath)).promise;
  cache.set(id, task);
  task.catch(() => {
    cache.delete(id);
    pinnedIds.delete(id);
  });
  evictOverflow();
  return task;
}

export async function destroyPdfDoc(id: string): Promise<void> {
  pinnedIds.delete(id);
  const task = cache.get(id);
  cache.delete(id);
  if (!task) return;
  try {
    await (await task).loadingTask.destroy();
  } catch {
  }
}
