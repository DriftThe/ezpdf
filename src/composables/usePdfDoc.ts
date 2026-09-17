import { convertFileSrc } from "@tauri-apps/api/core";
import { openPdf } from "../lib/pdfjs";
import type { PDFDocumentProxy } from "pdfjs-dist";

/** pdfjs doc cache keyed by stable PDF id, shared by both panes and the offscreen renderer.
 *  LRU ≤2: the pinned focused book is never evicted; background books go oldest-first.
 *  Always free via loadingTask.destroy() (PDFDocumentProxy.destroy() is gone in v6);
 *  a failed load clears the cache so it can be retried. */
const MAX_DOCS = 2;
/** Insertion-ordered Map as an LRU (touch = delete + reinsert, newest last). */
const cache = new Map<string, Promise<PDFDocumentProxy>>();
/** Pinned focused books — never evicted. */
const pinnedIds = new Set<string>();

function touch(id: string): void {
  const task = cache.get(id);
  if (!task) return;
  cache.delete(id);
  cache.set(id, task);
}

/** Evict over capacity: destroy unpinned docs oldest-first. */
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
  // PDF bytes never travel through invoke: convertFileSrc maps the absolute path to an
  // asset:// URL streamed by Rust's asset protocol under the repo scope; openPdf attaches
  // the CJK cMaps / standard fonts / wasm / ICC resource paths.
  const task = openPdf(convertFileSrc(pdfPath)).promise;
  cache.set(id, task);
  task.catch(() => {
    cache.delete(id);
    pinnedIds.delete(id);
  });
  evictOverflow();
  return task;
}

/** Free a doc via loadingTask.destroy() (also aborts in-flight load requests). */
export async function destroyPdfDoc(id: string): Promise<void> {
  pinnedIds.delete(id);
  const task = cache.get(id);
  cache.delete(id);
  if (!task) return;
  try {
    await (await task).loadingTask.destroy();
  } catch {
    // destroy failure is not fatal (e.g. a crashed worker)
  }
}
