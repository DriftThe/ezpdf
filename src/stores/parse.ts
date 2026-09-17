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

/** Page-level predicates shared by scheduler sampling. */
const needsOcr = (p: PageInfo): boolean => !p.finished;
const needsTranslation = (p: PageInfo): boolean => p.finished && !p.translated;

/** Types to translate: empty = built-in default (same fallback as Rust is_translatable). */
function effectiveTypes(): readonly string[] {
  const types = useSettingsStore().general.translateTypes;
  return types.length > 0 ? types : DEFAULT_TRANSLATED_TYPES;
}

/** Table backfill: pages translated before table support have table blocks with no
 *  translation and would never be revisited. Only tables whose grid Rust has parsed and
 *  persisted (parsable) and whose translation is empty qualify; other types are untouched. */
function needsTableBackfill(p: PageInfo): boolean {
  if (!p.finished || !effectiveTypes().includes("table")) return false;
  return p.blocks.some((b) => b.type === "table" && !b.translation && !!b.grid);
}

/** Translation sampling predicate: untranslated pages + pages with backfill tables. */
const needsWork = (p: PageInfo): boolean => needsTranslation(p) || needsTableBackfill(p);

export const useParseStore = defineStore("parse", () => {
  /** Global pause/resume. */
  const paused = ref(false);
  /** pyserver lifecycle (driven by ocr://status; unknown/disconnected/failed show gray/red). */
  const serviceStatus = ref<ServiceStatus>("unknown");
  /** Environment report (bootstrap JSON; null = not checked). */
  const envReport = ref<OcrEnvReport | null>(null);
  /** Service/install log stream (ocr://log; ring-capped to the last 200 lines). */
  const envLogs = ref<string[]>([]);
  /** Translation log stream (llm://log; ring-capped to 200 lines, shown in the LLM pane). */
  const llmLogs = ref<string[]>([]);

  const checking = ref(false);
  const installing = ref(false);
  /** Install progress (ocr://install; null = not installing). */
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

  // event subscriptions (once per singleton); non-Tauri silently fails
  listen<string>("ocr://log", (e) => pushLog(e.payload)).catch(() => undefined);
  listen<string>("llm://log", (e) => pushLlmLog(e.payload)).catch(() => undefined);
  listen<ServiceStatus>("ocr://status", (e) => {
    serviceStatus.value = e.payload;
  }).catch(() => undefined);
  listen<InstallProgress>("ocr://install", (e) => {
    installProgress.value = e.payload;
  }).catch(() => undefined);

  /** Check the environment: Rust runs bootstrap.py and caches the full report. */
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

  /** One-click install: env + torch variant → model download, progress via ocr://install.
   *  The backend skips already-installed envs (CPU ⊂ GPU) and aborts GPU mode without nvidia-smi. */
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

  /** Lifecycle commands (start/stop): toast on failure; status comes from ocr://status. */
  async function runServiceCommand(command: string): Promise<void> {
    try {
      await invoke(command);
    } catch (e) {
      toast(String(e), "error");
    }
  }

  const startService = (): Promise<void> => runServiceCommand("ocr_start");
  const stopService = (): Promise<void> => runServiceCommand("ocr_stop");

  /** Online handshake: read /health for the server-advertised batch size before every OCR
   *  request so config changes take effect. Throws on failure (never send with a stale number). */
  async function handshakeOnline(): Promise<ParseServiceHealth> {
    const ocr = useSettingsStore().ocr;
    const health = await invoke<ParseServiceHealth>("ocr_health", {
      url: ocr.url,
      token: ocr.token.trim(),
    });
    onlineHealth.value = health;
    return health;
  }

  /** Health report → localized toast text. */
  function healthDetail(h: ParseServiceHealth): string {
    return t("ocr.healthDetail", {
      pid: h.pid ?? "?",
      ms: h.elapsedMs,
      batch: h.maxBatchPages,
    });
  }

  /** Health report → one-line English log summary (logs are English everywhere). */
  function healthLine(h: ParseServiceHealth): string {
    return `pid ${h.pid ?? "-"} ${h.elapsedMs}ms max_batch_pages=${h.maxBatchPages}`;
  }

  /** Online connect (registered as the OCR target only after a health probe); throws on failure. */
  async function connectOnline(url: string): Promise<ParseServiceHealth> {
    const health = await invoke<ParseServiceHealth>("ocr_start_remote", {
      url,
      token: useSettingsStore().ocr.token.trim(),
    });
    onlineHealth.value = health;
    return health;
  }

  /** Start service: local spawns the child; online probes then registers the endpoint. */
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

  /** Health-probe only, without changing connection state. */
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

  /** Auto-wake on start: local needs env + models ready (else log only); online skips the
   *  local check and just probes + registers. */
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

  // ---- page-level OCR scheduling loop: parse_pdf bridge ----
  // The frontend schedules: one tick = one batch (same book) → Rust parse_pdf, one atomic write.
  // Event-driven: a finished batch continues the chain; an empty sweep goes standing.

  /** A batch in flight (re-entry guard). */
  const parsing = ref(false);
  /** Standing flag (no work); a wake event clears it. */
  const standing = ref(false);
  /** Wake arrived while a batch was in flight, taken over after it finishes. */
  let wakePending = false;
  /** Whether the pending wake includes an external event (only those reset strikes). */
  let wakePendingExternal = false;
  /** Local batch size (≤4 pages, same book). Online uses the server's advertised value. */
  const LOCAL_BATCH_SIZE = 4;
  /** Last online handshake (advertised batch size + pid/elapsed, shown in settings). */
  const onlineHealth = ref<ParseServiceHealth | null>(null);
  /** Pages per batch: local = constant; online = advertised (fallback 4). */
  function currentBatchSize(): number {
    if (useSettingsStore().ocr.mode !== "online") return LOCAL_BATCH_SIZE;
    return onlineHealth.value?.maxBatchPages ?? LOCAL_BATCH_SIZE;
  }
  /** First + 2 retries = 3 strikes → suspend that batch kind. */
  const MAX_ATTEMPTS = 3;
  /** Per-book strike counts, OCR and translation separate; cleared on an external wake. */
  const ocrStrikes = new Map<string, number>();
  const translateStrikes = new Map<string, number>();

  /** OCR precondition: parse service connected (local spawned / online registered). */
  function canOcr(): boolean {
    return serviceStatus.value === "connected";
  }

  /** Translation precondition: only LLM config, independent of the parse service.
   *  With translation off the payload is still non-null (bypass needs no keys). */
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

  /** Kick the loop (idempotent: a running chain continues by itself).
   *  external=false (self-continuation) keeps strike counts, so a broken LLM is not
   *  retried 3× per batch; real events (open/import/connect/resume) reset them. */
  function wake(external = true): void {
    if (!isTauri || paused.value) return;
    if (parsing.value) {
      wakePending = true; // in flight: the finally after this batch takes over, no lost wake
      wakePendingExternal = wakePendingExternal || external;
      return;
    }
    standing.value = false;
    if (external) clearStrikes();
    queueMicrotask(() => void tick());
  }

  /** One tick: pick a book → ring-collect a batch → offscreen render → parse_pdf → apply.
   *  Continues via queueMicrotask unless pickBook returns null (then standing). */
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
        standing.value = true; // sweep found nothing → wait for an event
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
    /** translate = finished-but-untranslated pages (priority over new OCR); ocr = unfinished pages. */
    kind: "ocr" | "translate";
  }

  /** Translation needs baseUrl/apiKey/model; the settings store assembles the compat snapshot. */
  function llmPayload(): LlmInvokePayload | null {
    return useSettingsStore().llmInvokePayload();
  }

  /** Pick a book: focused first, then index order; per-book load_pdf reads the JSON flags
   *  (no extra progress query command). Translation retries outrank new OCR — a finished but
   *  untranslated page may be an orphan from a restart or a failed translation. A saturated
   *  translation strike count suspends only that branch; OCR keeps running (and vice versa). */
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
      // OCR needs the parse service; skip while unavailable and wake once connected
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

  /** Book state: the focused book uses the store cache, background books read via load_pdf. */
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

  /** If a sweep ends with no work but the focused book needs translation and LLM is unset → notify once. */
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

  /** Ring start: the focused book from the current page, background books from page 1. */
  function startPageFor(book: PickTarget): number {
    const reader = useReaderStore();
    return book.focused
      ? Math.max(1, Math.min(reader.currentPage, book.state.pages.length))
      : 1;
  }

  /** Ring-collect ≤limit hits (pick null skips a page; limit defaults to the local batch size). */
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

  /** Process a batch: translation retry or OCR (translation queued alongside via queueTranslate). */
  async function processBatch(book: PickTarget): Promise<void> {
    if (book.kind === "translate") {
      await processTranslateBatch(book);
    } else {
      await processOcrBatch(book);
    }
  }

  /** Translation retry batch: finished but untranslated pages (ring, focused book from current). */
  async function processTranslateBatch(book: PickTarget): Promise<void> {
    const lib = useLibraryStore();
    const llm = llmPayload();
    if (!llm) throw new Error("LLM not configured (baseUrl/apiKey/model empty)");
    const root = lib.repoRoot;
    if (!root) throw new Error("no repository open");
    const pages = ringCollect(book, (page) => (needsWork(page) ? page.index : null));
    if (pages.length === 0) return; // race: all translated already
    pushLlmLog(`[ui] translation retry batch p${pages.join(",")} (${book.name})`);
    const outcome = await invokeTranslate(root, book.id, pages, llm);
    if (outcome.updatedPages.length === 0) {
      throw new Error("translation made no progress"); // count a strike, avoid spinning on a bad page
    }
  }

  /** OCR batch: ring-collect unfinished pages → offscreen render → parse_pdf → queue translation.
   *  Online renegotiates via /health first (a failed handshake counts as a batch failure). */
  async function processOcrBatch(book: PickTarget): Promise<void> {
    const lib = useLibraryStore();
    if (useSettingsStore().ocr.mode === "online") await handshakeOnline();
    const limit = currentBatchSize();
    const take = ringCollect(book, (page) => (needsOcr(page) ? page : null), limit);
    if (take.length === 0) return; // race: all finished already
    pushLlmLog(`[ui] OCR batch p${take.map((p) => p.index).join(",")} (${book.name})`);

    // offscreen render uses the Rust-resolved path (load_pdf validates it); never build name-id
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
    // translation is decoupled from the OCR pipeline: queue it as soon as the batch returns,
    // then move to the next OCR batch. Same-book translation is serial (Rust makes interior
    // pages concurrent) so contexts don't collide across batches.
    queueTranslate(book.id, take.map((p) => p.index));
  }

  /** Per-book translation chain: serial, failures don't block later batches, in-flight books are skipped. */
  const translateChains = new Map<string, Promise<boolean>>();

  function queueTranslate(bookId: string, pages: number[]): void {
    if (paused.value) return; // paused: leave untranslated pages to the retry branch
    const llm = llmPayload();
    if (!llm || pages.length === 0) return;
    // true = no progress (all failed / paused) — wake the retry branch after the chain drains
    const run = async (): Promise<boolean> => {
      if (paused.value) return true; // paused: skip, wake resumes it
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
      // untranslated pages left (failure/missed) → wake the retry branch; background books
      // have no cache, so fall back to noProgress. wake(false) keeps strike counts so a
      // broken LLM is not retried every batch.
      const cached = useLibraryStore().pdfs[bookId];
      if (noProgress || cached?.pages.some(needsWork)) wake(false);
    });
  }

  /** Call translate_pdf once and merge results into the store (shared by retry branch and chain). */
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

  /** Apply results: patch the cached focused book in place; background books are already
   *  written atomically by Rust and read fresh next round. */
  function applyOutcome(id: string, outcome: ParseOutcome): void {
    const lib = useLibraryStore();
    const cached = lib.pdfs[id];
    if (!cached) return;
    const updated = new Map(outcome.updatedPages.map((p) => [p.index, p]));
    cached.status = outcome.bookStatus;
    cached.pages = cached.pages.map((p) => updated.get(p.index) ?? p);
  }

  // ---- skeleton backfill on open: books lopdf could not read have empty pages;
  //      fill the measured count once pdfjs geometry is ready, then wake the scheduler ----
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
      wake(); // skeleton ready → run now
    } catch (e) {
      toast(String(e), "error");
    }
  }

  // service connected (reconnect or manual start) → wake the chain
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
