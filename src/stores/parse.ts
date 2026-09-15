import { defineStore } from "pinia";
import { ref, watch } from "vue";
import { toast } from "../composables/toast";
import type {
  InstallProgress,
  OcrEnvReport,
  PageInfo,
  ParseOutcome,
  ParsePageInput,
  PDF,
  PDFStruct,
  ServiceStatus,
} from "../types/domain";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { loadPdfDoc } from "../composables/usePdfDoc";
import { renderPageToDataUrl, RENDER_SCALE } from "../lib/pageCapture";
import { t } from "../lib/i18n";
import { isTauri } from "../lib/env";
import { useLibraryStore } from "./library";
import { useReaderStore } from "./reader";
import { useSettingsStore, type LlmInvokePayload } from "./settings";

/** 非 Tauri 环境（纯浏览器 pnpm dev）：invoke 必败，调度整体静默（同 listen().catch 哲学） */

/** 页级谓词（调度取样共用） */
const needsOcr = (p: PageInfo): boolean => !p.finished;
const needsTranslation = (p: PageInfo): boolean => p.finished && !p.translated;

export const useParseStore = defineStore("parse", () => {
  /** 全局暂停/恢复（阶段4由 Rust 调度器驱动） */
  const paused = ref(false);
  /** pyserver 生命周期状态（ocr://status 事件驱动；未知/断线/失败均显示灰/红） */
  const serviceStatus = ref<ServiceStatus>("unknown");
  /** 环境分层报告（ocr_env_report 的 bootstrap JSON；null = 未检查） */
  const envReport = ref<OcrEnvReport | null>(null);
  /** 服务/安装日志流（ocr://log；环形截断保尾 200 行） */
  const envLogs = ref<string[]>([]);
  /** 翻译链路日志流（llm://log；环形截断保尾 200 行，LLM 设置页底部展示） */
  const llmLogs = ref<string[]>([]);

  const checking = ref(false);
  const installing = ref(false);
  /** 一键安装服务进度（ocr://install；null = 未在安装） */
  const installProgress = ref<InstallProgress | null>(null);

  function togglePaused(): void {
    paused.value = !paused.value;
    toast(paused.value ? t("toast.parsePaused") : t("toast.parseResumed"), paused.value ? "warn" : "info");
    if (!paused.value) wake(); // 恢复 → 续链
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

  // 事件订阅（store 单例创建一次即完成；非 Tauri 环境（纯浏览器 dev）静默失败）
  listen<string>("ocr://log", (e) => pushLog(e.payload)).catch(() => undefined);
  listen<string>("llm://log", (e) => pushLlmLog(e.payload)).catch(() => undefined);
  listen<ServiceStatus>("ocr://status", (e) => {
    serviceStatus.value = e.payload;
  }).catch(() => undefined);
  listen<InstallProgress>("ocr://install", (e) => {
    installProgress.value = e.payload;
  }).catch(() => undefined);

  /** 检查环境：Rust 跑 bootstrap.py 探测，整份报告入缓存 */
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

  /** 一键安装服务（用户 2026-09-14）：环境+torch 变体（CPU/GPU 选择、镜像开关）
   *  → 模型下载；全程进度经 ocr://install 推送（按钮旁进度条）。
   *  已安装（含 CPU ⊂ GPU 子集规则）由后端判定并跳过 → 提示「服务已安装」；
   *  GPU 模式无 nvidia-smi 时后端直接报错中止（toast 展示原因）。 */
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
        if (alreadyEnv) pushLlmLog(t("log.envInstalledModelsOnly"));
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

  /** 生命周期命令（启动/停止）：失败 toast；状态由 ocr://status 事件回报 */
  async function runServiceCommand(command: string): Promise<void> {
    try {
      await invoke(command);
    } catch (e) {
      toast(String(e), "error");
    }
  }

  const startService = (): Promise<void> => runServiceCommand("ocr_start");
  const stopService = (): Promise<void> => runServiceCommand("ocr_stop");

  /** 启动自动唤醒（常规设置 autoLaunch，用户 2026-09-14）：环境/模型全就绪才拉起，
   *  缺件只记日志不打扰（到 OCR 服务设置页一键安装服务） */
  async function autoStartIfEnabled(): Promise<void> {
    if (!isTauri || !useSettingsStore().general.autoLaunch) return;
    await checkEnv();
    const r = envReport.value;
    const ready = !!r && !!r.python && r.missing.length === 0 && !!r.models?.layout && !!r.models?.vl;
    if (!ready) {
      pushLlmLog(t("log.autoWakeSkipped"));
      return;
    }
    pushLlmLog(t("log.autoWakeStart"));
    await startService();
  }

  // ---- OCR 页级调度回路（阶段4 批3，PLAN-OCR.md §4）：parse_pdf 桥 ----
  // 前端是调度者：单 tick = 一批（≤4 页、同书）→ Rust parse_pdf 整批推理 + 一次原子写。
  // 事件驱动链：本批完成 → 链式续跑；一轮扫描无可处理书 → standing 挂起，等事件唤醒。

  /** 一批在途（幂等门闩：重入 tick 直接返回） */
  const parsing = ref(false);
  /** 挂起标记（无可处理书）；wake 事件（开书/导入/连上/恢复）解除 */
  const standing = ref(false);
  /** 批在途时到达的 wake（翻译链清空/事件）——本批结束后接管，防止丢唤醒 */
  let wakePending = false;
  /** 上述 pending 是否含外部事件（外部才重置连败预算） */
  let wakePendingExternal = false;
  /** 批大小（用户拍板：一次 ≤4 页、同书，不足传剩余页） */
  const BATCH_SIZE = 4;
  /** 首次 + 重试 2 次 = 3 连败 → 本轮停该类批次 */
  const MAX_ATTEMPTS = 3;
  /** 书级连败计数（OCR / 翻译分开：翻译持续失败只关翻译支路，OCR 照跑；反之亦然）；wake 时清零 */
  const ocrStrikes = new Map<string, number>();
  const translateStrikes = new Map<string, number>();

  function isRunnable(): boolean {
    return (
      isTauri &&
      !paused.value &&
      !standing.value &&
      serviceStatus.value === "connected" &&
      !!useLibraryStore().repoRoot
    );
  }

  function clearStrikes(): void {
    ocrStrikes.clear();
    translateStrikes.clear();
  }

  /** 踢循环：standing=false 时链条自会续跑（重复踢无副作用）；
   *  服务中途断线杀掉的链条也靠它复活（不要求 standing=true）。
   *  external=false（翻译链清空的自我续跑）不清连败计数——否则 LLM 坏掉时
   *  每批次都会白试 3 次翻译；外部事件（开书/导入/连上/恢复）才重置预算 */
  function wake(external = true): void {
    if (!isTauri || paused.value) return;
    if (parsing.value) {
      wakePending = true; // 在途：本批结束后的 finally 接管，防止丢唤醒
      wakePendingExternal = wakePendingExternal || external;
      return;
    }
    standing.value = false;
    if (external) clearStrikes();
    queueMicrotask(() => void tick());
  }

  /** 单 tick：选书 → 环形收集一批未完成页 → 离屏渲染 → parse_pdf → 结果落地。
   *  finally 里链式续跑（queueMicrotask），无可处理 → pickBook 返回 null → standing 挂起 */
  async function tick(): Promise<void> {
    if (!isRunnable()) {
      if (isTauri) {
        pushLlmLog(
          t("log.tick", {
            svc: serviceStatus.value,
            paused: paused.value,
            standing: standing.value,
            repo: !!useLibraryStore().repoRoot,
          }),
        );
      }
      return;
    }
    parsing.value = true;
    try {
      const book = await pickBook();
      if (!book) {
        standing.value = true; // 一轮扫完无事可做 → 挂起等事件
        clearStrikes();
        pushLlmLog(t("log.sweepIdle"));
        notifyLlmMissingOnce();
        return;
      }
      try {
        await processBatch(book);
        (book.kind === "translate" ? translateStrikes : ocrStrikes).delete(book.id);
      } catch (err) {
        const kind = book.kind === "translate" ? t("pipeline.kindTranslate") : t("pipeline.kindOcr");
        const map = book.kind === "translate" ? translateStrikes : ocrStrikes;
        const n = (map.get(book.id) ?? 0) + 1;
        map.set(book.id, n);
        pushLlmLog(t("log.batchFailed", { kind, n, max: MAX_ATTEMPTS, name: book.name, err: String(err) }));
        if (n >= MAX_ATTEMPTS) {
          toast(
            book.kind === "translate"
              ? t("toast.translateStrikes", { name: book.name })
              : t("toast.ocrStrikes", { name: book.name }),
            "warn",
          );
        } else {
          console.warn(`[parse] 批次失败(${n}/${MAX_ATTEMPTS}) ${book.name}:`, err);
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
    /** translate = 已 OCR 未翻译页的补翻（优先于新 OCR）；ocr = 未 OCR 页 */
    kind: "ocr" | "translate";
  }

  /** LLM 三要素齐全才开翻译（否则整条翻译支路关闭，书直接视为无事可做）；
   *  预设兼容性快照（协议/关思考/请求字段）由 settings store 统一拼装 */
  function llmPayload(): LlmInvokePayload | null {
    return useSettingsStore().llmInvokePayload();
  }

  /** 选书：聚焦书优先，其余按索引序。逐本 load_pdf 直读绑定 JSON 的
   *  status/finished/translated 标志位（用户拍板：不加进度查询命令）。
   *  补翻优先（2026-09-14 修复）：finished && !translated 的页可能是上一轮
   *  OCR 落盘后翻译未落盘（重启丢内存配对）或翻译失败后的孤儿——早先要等
   *  整本 OCR 完才回头补翻，大书等于永不补；现在同书翻译链空闲就立刻补，
   *  链在途时不抢（OCR 批次已排好翻译，链路自会接续）。翻译连败达上限只关
   *  该书翻译支路，OCR 照跑（反之亦然） */
  async function pickBook(): Promise<PickTarget | null> {
    const lib = useLibraryStore();
    const index = lib.repoIndex;
    if (!index) return null;
    const focused = index.pdfs.find((p) => p.id === lib.currentPdfId) ?? null;
    const order = focused
      ? [focused, ...index.pdfs.filter((p) => p.id !== focused.id)]
      : [...index.pdfs];
    const canTranslate = llmPayload() !== null;
    for (const entry of order) {
      const state = await bookState(entry);
      if (!state || state.pages.length === 0) continue;
      const focusedBook = entry.id === lib.currentPdfId;
      if (
        canTranslate &&
        !translateChains.has(entry.id) &&
        (translateStrikes.get(entry.id) ?? 0) < MAX_ATTEMPTS &&
        state.pages.some(needsTranslation)
      ) {
        return { id: entry.id, name: entry.name, state, focused: focusedBook, kind: "translate" };
      }
      if ((ocrStrikes.get(entry.id) ?? 0) < MAX_ATTEMPTS && state.pages.some(needsOcr)) {
        return { id: entry.id, name: entry.name, state, focused: focusedBook, kind: "ocr" };
      }
    }
    return null;
  }

  /** 书状态：聚焦书用 store 缓存（批量写回已就地同步），后台书逐本 load_pdf 直读 */
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

  /** 一轮结束仍无书可跑、聚焦书有未翻译页但 LLM 未配置 → 提示一次（防无谓刷屏） */
  let llmMissingNotified = false;
  function notifyLlmMissingOnce(): void {
    if (llmMissingNotified || llmPayload()) return;
    const current = useLibraryStore().currentPdf;
    if (current?.pages.some(needsTranslation)) {
      llmMissingNotified = true;
      pushLlmLog(t("log.llmMissing"));
      toast(t("toast.llmMissing"), "warn");
    }
  }

  /** 环形取样起点：聚焦书从当前阅读页开始（用户视线先行），后台书从第 1 页 */
  function startPageFor(book: PickTarget): number {
    const reader = useReaderStore();
    return book.focused
      ? Math.max(1, Math.min(reader.currentPage, book.state.pages.length))
      : 1;
  }

  /** 环形收集 ≤BATCH_SIZE 个命中页（pick 返回 null = 跳过该页） */
  function ringCollect<T>(book: PickTarget, pick: (page: PageInfo) => T | null): T[] {
    const pages = book.state.pages;
    const start = startPageFor(book);
    const out: T[] = [];
    for (let k = 0; k < pages.length && out.length < BATCH_SIZE; k++) {
      const hit = pick(pages[(start - 1 + k) % pages.length]);
      if (hit !== null) out.push(hit);
    }
    return out;
  }

  /** 处理一批：翻译重试（translate_pdf）或 OCR（翻译由 queueTranslate 并发跟进） */
  async function processBatch(book: PickTarget): Promise<void> {
    if (book.kind === "translate") {
      await processTranslateBatch(book);
    } else {
      await processOcrBatch(book);
    }
  }

  /** 翻译重试批次：finished && !translated 的页（环形，聚焦书从当前页起） */
  async function processTranslateBatch(book: PickTarget): Promise<void> {
    const lib = useLibraryStore();
    const llm = llmPayload();
    if (!llm) throw new Error(t("pipeline.errLlmNotConfigured"));
    const pages = ringCollect(book, (page) => (needsTranslation(page) ? page.index : null));
    if (pages.length === 0) return; // 竞态：已全部翻译
    pushLlmLog(t("log.translateRetryBatch", { pages: pages.join(","), name: book.name }));
    const outcome = await invoke<ParseOutcome>("translate_pdf", {
      root: lib.repoRoot,
      id: book.id,
      pages,
      llm,
    });
    if (outcome.updatedPages.length === 0) {
      throw new Error(t("pipeline.errNoProgress")); // 计入 strike，防止坏页空转
    }
    applyOutcome(book.id, outcome);
  }

  /** OCR 批次：环形取 ≤BATCH_SIZE 未完成页 → 离屏渲染 → parse_pdf → 排翻译链 */
  async function processOcrBatch(book: PickTarget): Promise<void> {
    const lib = useLibraryStore();
    const take = ringCollect(book, (page) => (needsOcr(page) ? page : null));
    if (take.length === 0) return; // 竞态：已全部完成
    pushLlmLog(t("log.ocrBatch", { pages: take.map((p) => p.index).join(","), name: book.name }));

    // 离屏渲染：一律用 Rust 解析过的路径（load_pdf 已做仓库内校验），不自己拼 name-id
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
    // 翻译与 pipeline 解耦（用户拍板 2026-09-14）：OCR 批一返回就把翻译排入
    // 本书翻译链，调度链立刻去下一批 OCR；同书翻译串行（Rust 内隔页并发），
    // 避免跨批上下文互踩
    queueTranslate(book.id, take.map((p) => p.index));
  }

  /** 书级翻译链：同书排队串行、失败不阻塞后续批次；在途时 pickBook 不再选该书翻译 */
  const translateChains = new Map<string, Promise<boolean>>();

  function queueTranslate(bookId: string, pages: number[]): void {
    if (paused.value) return; // 暂停：不排新翻译任务（未翻页留给恢复后的重试支路）
    const llm = llmPayload();
    if (!llm || pages.length === 0) return;
    // 返回 true = 整批无进展（全失败/暂停跳过）——链清空后需要唤醒重试支路
    const run = async (): Promise<boolean> => {
      if (paused.value) return true; // 暂停：跳过本批，恢复时由 wake 续跑
      const root = useLibraryStore().repoRoot;
      if (!root) return false;
      const outcome = await invoke<ParseOutcome>("translate_pdf", {
        root,
        id: bookId,
        pages,
        llm,
      });
      applyOutcome(bookId, outcome);
      return outcome.updatedPages.length === 0;
    };
    const prev = translateChains.get(bookId) ?? Promise.resolve();
    const next = prev
      .catch(() => undefined)
      .then(run)
      .catch((err) => {
        pushLlmLog(t("log.translateBatchFailed", { pages: pages.join(","), err: String(err) }));
        return true;
      });
    translateChains.set(bookId, next);
    void next.then((noProgress) => {
      if (translateChains.get(bookId) !== next) return;
      translateChains.delete(bookId);
      // 仍有未翻译页（失败/漏批）→ 唤醒补翻支路（translateStrikes 防打转）；
      // 后台书无缓存，用 noProgress（整批无进展）兜底。自我续跑用 wake(false)：
      // 不重置连败预算，避免 LLM 坏掉时每批白试
      const cached = useLibraryStore().pdfs[bookId];
      if (noProgress || cached?.pages.some(needsTranslation)) wake(false);
    });
  }

  /** 批量结果落地：聚焦书（有缓存）就地 patch（译文栏响应式刷新）；后台书无缓存，
   *  磁盘 JSON 已由 Rust 原子写回，下轮 pickBook 直读即见 */
  function applyOutcome(id: string, outcome: ParseOutcome): void {
    const lib = useLibraryStore();
    const cached = lib.pdfs[id];
    if (!cached) return;
    const updated = new Map(outcome.updatedPages.map((p) => [p.index, p]));
    cached.status = outcome.bookStatus;
    cached.pages = cached.pages.map((p) => updated.get(p.index) ?? p);
  }

  // ---- 打开书补骨架（用户拍板）：lopdf 解析失败的书 pages 为空，
  //      pdfjs 几何就绪后以实测页数回填，再唤醒调度 ----
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
      if (lib.currentPdfId !== id) return; // 已切书：JSON 已补，store 无需动
      const loaded = await invoke<PDF>("load_pdf", { root: lib.repoRoot, id });
      lib.pdfs[id] = { ...loaded, id };
      wake(); // 骨架就位 → 立即开跑
    } catch (e) {
      toast(String(e), "error");
    }
  }

  // 服务连上（含断线重连/手动启动成功）→ 唤醒调度链
  watch(serviceStatus, (s) => {
    if (s === "connected") wake();
  });

  return {
    paused,
    serviceStatus,
    togglePaused,
    envReport,
    envLogs,
    llmLogs,
    checking,
    installing,
    installProgress,
    wake,
    checkEnv,
    installService,
    startService,
    stopService,
    autoStartIfEnabled,
  };
});
