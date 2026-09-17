import { defineStore } from "pinia";
import { ref, watch } from "vue";
import { toast } from "../composables/toast";
import type {
  InstallProgress,
  ParseServiceHealth,
  OcrEnvReport,
  PageInfo,
  ParseOutcome,
  ParsePageInput,
  PDF,
  ServiceStatus,
} from "../types/domain";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { loadPdfDoc } from "../composables/usePdfDoc";
import { renderPageToDataUrl, RENDER_SCALE } from "../lib/pageCapture";
import { t } from "../lib/i18n";
import { isTauri } from "../lib/env";
import { DEFAULT_TRANSLATED_TYPES } from "../lib/blocks";
import { useLibraryStore } from "./library";
import { useReaderStore } from "./reader";
import type { PDFStruct } from "../../src-tauri/bindings/PDFStruct";
import { useSettingsStore, type LlmInvokePayload } from "./settings";

/** Non-Tauri (browser pnpm dev): invoke always fails, so the whole scheduler stays silent. */

const needsOcr = (p: PageInfo): boolean => !p.finished;
const needsTranslation = (p: PageInfo): boolean => p.finished && !p.translated;

/** Empty set = built-in default (same fallback as Rust). */
function effectiveTypes(): readonly string[] {
  const types = useSettingsStore().general.translateTypes;
  return types.length > 0 ? types : DEFAULT_TRANSLATED_TYPES;
}

/** Backfill tables translated before table support (parsed grid, no translation); other types untouched. */
function needsTableBackfill(p: PageInfo): boolean {
  if (!p.finished || !effectiveTypes().includes("table")) return false;
  return p.blocks.some((b) => b.type === "table" && !b.translation && !!b.grid);
}

const needsWork = (p: PageInfo): boolean => needsTranslation(p) || needsTableBackfill(p);

export const useParseStore = defineStore("parse", () => {
  const paused = ref(false);
  const serviceStatus = ref<ServiceStatus>("unknown");
  const envReport = ref<OcrEnvReport | null>(null);
  const envLogs = ref<string[]>([]);
  const llmLogs = ref<string[]>([]);

  const checking = ref(false);
  const installing = ref(false);
  const installProgress = ref<InstallProgress | null>(null);

  function togglePaused(): void {
    paused.value = !paused.value;
    toast(paused.value ? t("toast.parsePaused") : t("toast.parseResumed"), paused.value ? "warn" : "info");
    if (!paused.value) wake(); // resumed → continue the chain
  }

  function pushCapped(target: { value: string[] }, line: string): void {
    target.value.push(line);
    if (target.value.length > 200) {
      target.value.splice(0, target.value.length - 200);
    }
  }

  function pushLog(line: string): void {
    pushCapped(envLogs, line);
  }

  function pushLlmLog(line: string): void {
    pushCapped(llmLogs, line);
  }

  // once per singleton; listen() rejects silently outside Tauri
  listen<string>("ocr://log", (e) => pushLog(e.payload)).catch(() => undefined);
  listen<string>("llm://log", (e) => pushLlmLog(e.payload)).catch(() => undefined);
  listen<ServiceStatus>("ocr://status", (e) => {
    serviceStatus.value = e.payload;
  }).catch(() => undefined);
  listen<InstallProgress>("ocr://install", (e) => {
    installProgress.value = e.payload;
  }).catch(() => undefined);

  async function checkEnv(): Promise<void> {
    checking.value = true;
    try {
      envReport.value = await invoke<OcrEnvReport>("ocr_env_report");
      const err = envReport.value?.error;
      if (err) toast(err, "warn");
    } catch (e) {
      toast(String(e), "error");
    } finally {
      checking.value = false;
    }
  }

  async function installService(mode: "cpu" | "gpu", useMirror: boolean): Promise<void> {
    if (installing.value) return;
    installing.value = true;
    installProgress.value = null;
    try {
      const alreadyEnv = await invoke<boolean>("ocr_install_env", { mode, useMirror });
      await checkEnv();
      const r = envReport.value;
      const modelsReady = !!r?.models?.layout && !!r?.models?.vl;
      if (!modelsReady) {
        if (alreadyEnv) pushLlmLog("[ui] environment already installed, only fetching models");
        await invoke("ocr_download_models", { useMirror });
        await checkEnv();
        toast(t("toast.serviceInstallDone"), "info");
      } else if (alreadyEnv) {
        toast(t("toast.serviceInstalled"), "info");
      } else {
        toast(t("toast.serviceInstallDone"), "info");
      }
    } catch (e) {
      toast(String(e), "error");
    } finally {
      installing.value = false;
      installProgress.value = null;
    }
  }

  async function runServiceCommand(command: string): Promise<void> {
    try {
      await invoke(command);
    } catch (e) {
      toast(String(e), "error");
    }
  }

  const startService = (): Promise<void> => runServiceCommand("ocr_start");
  const stopService = (): Promise<void> => runServiceCommand("ocr_stop");

  /** Re-read /health before every online batch so a changed batch size takes effect. */
  async function handshakeOnline(): Promise<ParseServiceHealth> {
    const ocr = useSettingsStore().ocr;
    const health = await invoke<ParseServiceHealth>("ocr_health", {
      url: ocr.url,
      token: ocr.token.trim(),
    });
    onlineHealth.value = health;
    return health;
  }

  function healthDetail(h: ParseServiceHealth): string {
    return t("ocr.healthDetail", {
      pid: h.pid ?? "?",
      ms: h.elapsedMs,
      batch: h.maxBatchPages,
    });
  }

  function healthLine(h: ParseServiceHealth): string {
    return `pid ${h.pid ?? "-"} ${h.elapsedMs}ms max_batch_pages=${h.maxBatchPages}`;
  }

  async function connectOnline(url: string): Promise<ParseServiceHealth> {
    const health = await invoke<ParseServiceHealth>("ocr_start_remote", {
      url,
      token: useSettingsStore().ocr.token.trim(),
    });
    onlineHealth.value = health;
    return health;
  }

  async function startServiceForMode(): Promise<void> {
    if (useSettingsStore().ocr.mode !== "online") {
      await startService();
      return;
    }
    try {
      const h = await connectOnline(useSettingsStore().ocr.url);
      pushLlmLog(`[ui] online parse service connected: ${healthLine(h)}`);
    } catch (e) {
      toast(String(e), "error");
    }
  }

  async function testRemote(): Promise<void> {
    try {
      const h = await handshakeOnline();
      pushLlmLog(`[ui] online parse service health check ok: ${healthLine(h)}`);
      toast(t("toast.parseServiceOk", { detail: healthDetail(h) }), "info");
    } catch (e) {
      onlineHealth.value = null;
      pushLlmLog(`[ui] online parse service health check failed: ${String(e)}`);
      toast(String(e), "error");
    }
  }

  async function autoStartIfEnabled(): Promise<void> {
    const settings = useSettingsStore();
    if (!isTauri || !settings.general.autoLaunch) return;
    if (settings.ocr.mode === "online") {
      try {
        const h = await connectOnline(settings.ocr.url);
        pushLlmLog(`[ui] online parse service connected: ${healthLine(h)}`);
      } catch (e) {
        pushLlmLog(`[ui] failed to connect the online parse service: ${String(e)}`);
      }
      return;
    }
    await checkEnv();
    const r = envReport.value;
    const ready = !!r && !!r.python && r.missing.length === 0 && !!r.models?.layout && !!r.models?.vl;
    if (!ready) {
      pushLlmLog("[ui] OCR auto-wake skipped: environment or models not ready (Settings → OCR service)");
      return;
    }
    pushLlmLog("[ui] OCR auto-wake: environment ready → starting service");
    await startService();
  }

  const parsing = ref(false);
  const standing = ref(false);
  /** Wake arrived mid-batch, taken over after it finishes; external wakes reset strikes. */
  let wakePending = false;
  let wakePendingExternal = false;
  const LOCAL_BATCH_SIZE = 4;
  /** Server-advertised batch size from the last handshake (settings display too). */
  const onlineHealth = ref<ParseServiceHealth | null>(null);
  function currentBatchSize(): number {
    if (useSettingsStore().ocr.mode !== "online") return LOCAL_BATCH_SIZE;
    return onlineHealth.value?.maxBatchPages ?? LOCAL_BATCH_SIZE;
  }
  /** First + 2 retries = 3 strikes. */
  const MAX_ATTEMPTS = 3;
  /** Per-book strikes, OCR and translation separate; an external wake clears them. */
  const ocrStrikes = new Map<string, number>();
  const translateStrikes = new Map<string, number>();

  function canOcr(): boolean {
    return serviceStatus.value === "connected";
  }

  /** Translation needs only LLM config; bypass (translation off) still sends a keyless payload. */
  function canTranslate(): boolean {
    return llmPayload() !== null;
  }

  function isRunnable(): boolean {
    return (
      isTauri &&
      !paused.value &&
      !standing.value &&
      !!useLibraryStore().repoRoot &&
      (canOcr() || canTranslate())
    );
  }

  function clearStrikes(): void {
    ocrStrikes.clear();
    translateStrikes.clear();
  }

  /** Kick the loop; external events (open/import/connect/resume) reset strikes, self-continuation keeps them. */
  function wake(external = true): void {
    if (!isTauri || paused.value) return;
    if (parsing.value) {
      wakePending = true;
      wakePendingExternal = wakePendingExternal || external;
      return;
    }
    standing.value = false;
    if (external) clearStrikes();
    queueMicrotask(() => void tick());
  }

  async function tick(): Promise<void> {
    if (!isRunnable()) {
      if (isTauri) {
        pushLlmLog(
          `[ui] scheduler not ready (service=${serviceStatus.value} paused=${paused.value} ` +
            `standing=${standing.value} repo=${!!useLibraryStore().repoRoot})`,
        );
      }
      return;
    }
    parsing.value = true;
    try {
      const book = await pickBook();
      if (!book) {
        standing.value = true;
        clearStrikes();
        pushLlmLog("[ui] sweep done: no processable pages → standing");
        notifyLlmMissingOnce();
        return;
      }
      try {
        await processBatch(book);
        (book.kind === "translate" ? translateStrikes : ocrStrikes).delete(book.id);
      } catch (err) {
        const kind = book.kind === "translate" ? "translation" : "OCR";
        const map = book.kind === "translate" ? translateStrikes : ocrStrikes;
        const n = (map.get(book.id) ?? 0) + 1;
        map.set(book.id, n);
        pushLlmLog(`[ui] ${kind} batch failed (${n}/${MAX_ATTEMPTS}) ${book.name}: ${String(err)}`);
        if (n >= MAX_ATTEMPTS) {
          toast(
            book.kind === "translate"
              ? t("toast.translateStrikes", { name: book.name })
              : t("toast.ocrStrikes", { name: book.name }),
            "warn",
          );
        } else {
          console.warn(`[parse] batch failed (${n}/${MAX_ATTEMPTS}) ${book.name}:`, err);
        }
      }
    } finally {
      parsing.value = false;
    }
    if (wakePending) {
      wakePending = false;
      standing.value = false;
      if (wakePendingExternal) clearStrikes();
      wakePendingExternal = false;
    }
    if (!standing.value) queueMicrotask(() => void tick());
  }

  interface PickTarget {
    id: string;
    name: string;
    state: PDF;
    focused: boolean;
    kind: "ocr" | "translate";
  }

  function llmPayload(): LlmInvokePayload | null {
    return useSettingsStore().llmInvokePayload();
  }

  async function pickBook(): Promise<PickTarget | null> {
    const lib = useLibraryStore();
    const index = lib.repoIndex;
    if (!index) return null;
    const focused = index.pdfs.find((p) => p.id === lib.currentPdfId) ?? null;
    const order = focused
      ? [focused, ...index.pdfs.filter((p) => p.id !== focused.id)]
      : [...index.pdfs];
    const canTranslateNow = canTranslate();
    for (const entry of order) {
      const state = await bookState(entry);
      if (!state || state.pages.length === 0) continue;
      const focusedBook = entry.id === lib.currentPdfId;
      if (
        canTranslateNow &&
        !translateChains.has(entry.id) &&
        (translateStrikes.get(entry.id) ?? 0) < MAX_ATTEMPTS &&
        state.pages.some(needsWork)
      ) {
        return { id: entry.id, name: entry.name, state, focused: focusedBook, kind: "translate" };
      }
      if (
        canOcr() &&
        (ocrStrikes.get(entry.id) ?? 0) < MAX_ATTEMPTS &&
        state.pages.some(needsOcr)
      ) {
        return { id: entry.id, name: entry.name, state, focused: focusedBook, kind: "ocr" };
      }
    }
    return null;
  }

  /** Focused book from the store cache; background books re-read via load_pdf. */
  async function bookState(entry: PDFStruct): Promise<PDF | null> {
    const lib = useLibraryStore();
    if (entry.id === lib.currentPdfId && lib.currentPdf) return lib.currentPdf;
    if (!lib.repoRoot) return null;
    try {
      return await invoke<PDF>("load_pdf", { root: lib.repoRoot, id: entry.id });
    } catch {
      return null;
    }
  }

  let llmMissingNotified = false;
  function notifyLlmMissingOnce(): void {
    if (llmMissingNotified || llmPayload()) return;
    const current = useLibraryStore().currentPdf;
    if (current?.pages.some(needsWork)) {
      llmMissingNotified = true;
      pushLlmLog("[ui] LLM not configured (baseUrl/apiKey/model empty) → translation skipped; fill it in Settings");
      toast(t("toast.llmMissing"), "warn");
    }
  }

  function startPageFor(book: PickTarget): number {
    const reader = useReaderStore();
    return book.focused
      ? Math.max(1, Math.min(reader.currentPage, book.state.pages.length))
      : 1;
  }

  /** Collect ≤limit hits in a ring starting at startPageFor. */
  function ringCollect<T>(
    book: PickTarget,
    pick: (page: PageInfo) => T | null,
    limit: number = LOCAL_BATCH_SIZE,
  ): T[] {
    const pages = book.state.pages;
    const start = startPageFor(book);
    const out: T[] = [];
    for (let k = 0; k < pages.length && out.length < limit; k++) {
      const hit = pick(pages[(start - 1 + k) % pages.length]);
      if (hit !== null) out.push(hit);
    }
    return out;
  }

  async function processBatch(book: PickTarget): Promise<void> {
    if (book.kind === "translate") {
      await processTranslateBatch(book);
    } else {
      await processOcrBatch(book);
    }
  }

  async function processTranslateBatch(book: PickTarget): Promise<void> {
    const lib = useLibraryStore();
    const llm = llmPayload();
    if (!llm) throw new Error("LLM not configured (baseUrl/apiKey/model empty)");
    const root = lib.repoRoot;
    if (!root) throw new Error("no repository open");
    const pages = ringCollect(book, (page) => (needsWork(page) ? page.index : null));
    if (pages.length === 0) return;
    pushLlmLog(`[ui] translation retry batch p${pages.join(",")} (${book.name})`);
    const outcome = await invokeTranslate(root, book.id, pages, llm);
    if (outcome.updatedPages.length === 0) {
      throw new Error("translation made no progress");
    }
  }

  /** A failed online handshake counts as a batch failure. */
  async function processOcrBatch(book: PickTarget): Promise<void> {
    const lib = useLibraryStore();
    if (useSettingsStore().ocr.mode === "online") await handshakeOnline();
    const limit = currentBatchSize();
    const take = ringCollect(book, (page) => (needsOcr(page) ? page : null), limit);
    if (take.length === 0) return;
    pushLlmLog(`[ui] OCR batch p${take.map((p) => p.index).join(",")} (${book.name})`);

    // offscreen render uses the Rust-resolved path; never build name-id
    const pdfPath = book.state.pdfPath;
    const doc = await loadPdfDoc(book.id, pdfPath);
    const pages: ParsePageInput[] = [];
    for (const page of take) {
      pages.push({
        index: page.index,
        imageB64: await renderPageToDataUrl(doc, page.index),
        scale: RENDER_SCALE,
      });
    }
    const outcome = await invoke<ParseOutcome>("parse_pdf", {
      root: lib.repoRoot,
      id: book.id,
      pages,
    });
    applyOutcome(book.id, outcome);
    // decoupled: queue translation as soon as the batch returns, then start the next OCR batch
    queueTranslate(book.id, take.map((p) => p.index));
  }

  /** Per-book serial chain; failures don't block later batches, in-flight books are skipped. */
  const translateChains = new Map<string, Promise<boolean>>();

  function queueTranslate(bookId: string, pages: number[]): void {
    if (paused.value) return;
    const llm = llmPayload();
    if (!llm || pages.length === 0) return;
    // true = no progress → wake the retry branch after the chain drains
    const run = async (): Promise<boolean> => {
      if (paused.value) return true;
      const root = useLibraryStore().repoRoot;
      if (!root) return false;
      const outcome = await invokeTranslate(root, bookId, pages, llm);
      return outcome.updatedPages.length === 0;
    };
    const prev = translateChains.get(bookId) ?? Promise.resolve();
    const next = prev
      .catch(() => undefined)
      .then(run)
      .catch((err) => {
        pushLlmLog(`[ui] translation batch failed p${pages.join(",")}: ${String(err)}`);
        return true;
      });
    translateChains.set(bookId, next);
    void next.then((noProgress) => {
      if (translateChains.get(bookId) !== next) return;
      translateChains.delete(bookId);
      // wake(false) keeps strikes so a broken LLM isn't retried every batch
      const cached = useLibraryStore().pdfs[bookId];
      if (noProgress || cached?.pages.some(needsWork)) wake(false);
    });
  }

  async function invokeTranslate(
    root: string,
    bookId: string,
    pages: number[],
    llm: LlmInvokePayload,
  ): Promise<ParseOutcome> {
    const outcome = await invoke<ParseOutcome>("translate_pdf", { root, id: bookId, pages, llm });
    applyOutcome(bookId, outcome);
    return outcome;
  }

  /** Patch the cached focused book; background books are written by Rust and re-read next round. */
  function applyOutcome(id: string, outcome: ParseOutcome): void {
    const lib = useLibraryStore();
    const cached = lib.pdfs[id];
    if (!cached) return;
    const updated = new Map(outcome.updatedPages.map((p) => [p.index, p]));
    cached.status = outcome.bookStatus;
    cached.pages = cached.pages.map((p) => updated.get(p.index) ?? p);
  }

  // books lopdf could not read have empty pages; fill the pdfjs count once geometry is ready
  watch(
    () => {
      const lib = useLibraryStore();
      return [useReaderStore().numPages, lib.currentPdf?.pages.length ?? 0, lib.currentPdfId] as const;
    },
    ([numPages, pageLen, pdfId]) => {
      if (pdfId && numPages > 0 && pageLen === 0) void prefillCurrent(pdfId, numPages);
    },
  );

  async function prefillCurrent(id: string, numPages: number): Promise<void> {
    const lib = useLibraryStore();
    if (!lib.repoRoot) return;
    try {
      await invoke("prefill_pages", { root: lib.repoRoot, id, total: numPages });
      if (lib.currentPdfId !== id) return; // switched books: JSON filled, store untouched
      await lib.loadPdf(id);
      wake();
    } catch (e) {
      toast(String(e), "error");
    }
  }

  // service connected → wake the chain
  watch(serviceStatus, (s) => {
    if (s === "connected") wake();
  });

  return {
    paused,
    parsing,
    serviceStatus,
    togglePaused,
    envReport,
    envLogs,
    llmLogs,
    onlineHealth,
    checking,
    installing,
    installProgress,
    wake,
    checkEnv,
    installService,
    startService: startServiceForMode,
    stopService,
    testRemote,
    autoStartIfEnabled,
  };
});
