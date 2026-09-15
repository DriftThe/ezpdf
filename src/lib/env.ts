/** Tauri 运行时探测：浏览器里跑 `pnpm dev` 时为 false（IPC 相关代码需整体跳过）。 */
export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
