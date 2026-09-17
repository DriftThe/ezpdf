/** Tauri runtime probe: false under browser `pnpm dev` (skip all IPC code). */
export const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
