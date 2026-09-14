import { defineStore } from "pinia";
import { ref, watch } from "vue";
import { toast } from "../composables/toast";
import type {
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
import { useLibraryStore } from "./library";
import { useReaderStore } from "./reader";
import { useSettingsStore } from "./settings";

/** invoke 传给 Rust 的 LLM 配置（translate.rs LlmConfig，serde camelCase） */
interface LlmPayload {
  baseUrl: string;
  apiKey: string;
  model: string;
  targetLang: string;
  smartContext: boolean;
  thinkingOff: string;
}

/** 非 Tauri 环境（纯浏览器 pnpm dev）：invoke 必败，调度整体静默（同 listen().catch 哲学） */
const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

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
  const modelsBusy = ref(false);

  function togglePaused(): void {
    paused.value = !paused.value;
    toast(paused.value ? "解析已暂停" : "解析已恢复", paused.value ? "warn" : "info");
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

  /** 长任务命令（环境安装/模型下载）：busy 防重入 → invoke → 重查环境 → 成功提示 */
  async function runLongCommand(command: string, busy: { value: boolean }, success: string): Promise<void> {
    if (busy.value) return;
    busy.value = true;
    try {
      await invoke(command);
      await checkEnv();
      toast(success, "info");
    } catch (e) {
      toast(String(e), "error");
    } finally {
      busy.value = false;
    }
  }

  /** 一键安装：venv 创建（必要时）→ 基础依赖 → torch 变体；过程经 ocr://log 流式展示 */
  function installEnv(): Promise<void> {
    return runLongCommand("ocr_install_env", installing, "环境安装完成");
  }

  /** 模型下载：huggingface_hub 按需补装 → python -m app.fetch（hf-mirror 镜像，缺哪补哪） */
  function downloadModels(): Promise<void> {
    return runLongCommand("ocr_download_models", modelsBusy, "模型下载完成");
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
      pushLlmLog("[ui] OCR 自动唤醒跳过：环境或模型未就绪（设置 → OCR 服务）");
      return;
    }
    pushLlmLog("[ui] OCR 自动唤醒：环境就绪 → 启动服务");
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
  /** 批大小（用户拍板：一次 ≤4 页、同书，不足传剩余页） */
  const BATCH_SIZE = 4;
  /** 首次 + 重试 2 次 = 3 连败 → 本轮跳过该书 */
  const MAX_ATTEMPTS = 3;
  /** 书级连败计数；wake 时清零 */
  const strikes = new Map<string, number>();

  function isRunnable(): boolean {
    return (
      isTauri &&
      !paused.value &&
      !standing.value &&
      serviceStatus.value === "connected" &&
      !!useLibraryStore().repoRoot
    );
  }

  /** 踢循环：standing=false 时链条自会续跑（重复踢无副作用）；
   *  服务中途断线杀掉的链条也靠它复活（不要求 standing=true） */
  function wake(): void {
    if (!isTauri || paused.value) return;
    if (parsing.value) {
      wakePending = true; // 在途：本批结束后的 finally 接管，防止丢唤醒
      return;
    }
    standing.value = false;
    strikes.clear();
    queueMicrotask(() => void tick());
  }

  /** 单 tick：选书 → 环形收集一批未完成页 → 离屏渲染 → parse_pdf → 结果落地。
   *  finally 里链式续跑（queueMicrotask），无可处理 → pickBook 返回 null → standing 挂起 */
  async function tick(): Promise<void> {
    if (!isRunnable()) {
      if (isTauri) {
        pushLlmLog(
          `[ui] 调度未就绪（服务=${serviceStatus.value} 暂停=${paused.value} 挂起=${standing.value} 仓库=${!!useLibraryStore().repoRoot}）`,
        );
      }
      return;
    }
    parsing.value = true;
    try {
      const book = await pickBook();
      if (!book) {
        standing.value = true; // 一轮扫完无事可做 → 挂起等事件
        strikes.clear();
        pushLlmLog("[ui] 一轮扫完：无可处理页 → 挂起");
        notifyLlmMissingOnce();
        return;
      }
      try {
        await processBatch(book);
        strikes.delete(book.id);
      } catch (err) {
        const n = (strikes.get(book.id) ?? 0) + 1;
        strikes.set(book.id, n);
        const kind = book.kind === "translate" ? "翻译" : "OCR";
        pushLlmLog(`[ui] ${kind}批次失败(${n}/${MAX_ATTEMPTS}) ${book.name}: ${String(err)}`);
        if (n >= MAX_ATTEMPTS) {
          toast(`《${book.name}》解析连续失败，本轮跳过`, "warn");
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
      strikes.clear();
    }
    if (!standing.value) queueMicrotask(() => void tick());
  }

  interface PickTarget {
    id: string;
    name: string;
    state: PDF;
    focused: boolean;
    /** ocr = 有未 OCR 页优先做；translate = OCR 已完但存在未翻译页 */
    kind: "ocr" | "translate";
  }

  /** LLM 三要素齐全才开翻译（否则整条翻译支路关闭，书直接视为无事可做） */
  function llmPayload(): LlmPayload | null {
    const s = useSettingsStore().llm;
    if (!s.baseUrl.trim() || !s.apiKey.trim() || !s.model.trim()) return null;
    return {
      baseUrl: s.baseUrl.trim(),
      apiKey: s.apiKey,
      model: s.model,
      targetLang: s.targetLang,
      smartContext: s.smartContext,
      thinkingOff: s.thinkingOff,
    };
  }

  /** 选书：聚焦书优先，其余按索引序。逐本 load_pdf 直读绑定 JSON 的
   *  status/finished/translated 标志位（用户拍板：不加进度查询命令）；
   *  OCR 未完 → 先 OCR；OCR 完但有未翻译页（且 LLM 已配置）→ 翻译重试支路；
   *  页数未知空骨架书（待打开补骨架）/ 连败黑名单 → 跳过 */
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
      if ((strikes.get(entry.id) ?? 0) >= MAX_ATTEMPTS) continue;
      const state = await bookState(entry);
      if (!state || state.pages.length === 0) continue;
      const focusedBook = entry.id === lib.currentPdfId;
      if (state.pages.some(needsOcr)) {
        return { id: entry.id, name: entry.name, state, focused: focusedBook, kind: "ocr" };
      }
      if (canTranslate && !translateChains.has(entry.id) && state.pages.some(needsTranslation)) {
        return { id: entry.id, name: entry.name, state, focused: focusedBook, kind: "translate" };
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
      pushLlmLog("[ui] LLM 未配置（baseUrl/apiKey/model 为空）→ 翻译跳过；到设置页填写");
      toast("LLM 未配置，翻译已跳过（设置页填写或配置 auth.cfg）", "warn");
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
    if (!llm) throw new Error("LLM 未配置");
    const pages = ringCollect(book, (page) => (needsTranslation(page) ? page.index : null));
    if (pages.length === 0) return; // 竞态：已全部翻译
    pushLlmLog(`[ui] 翻译重试批次 p${pages.join(",")}（${book.name}）`);
    const outcome = await invoke<ParseOutcome>("translate_pdf", {
      root: lib.repoRoot,
      id: book.id,
      pages,
      llm,
    });
    if (outcome.updatedPages.length === 0) {
      throw new Error("翻译无进展"); // 计入 strike，防止坏页空转
    }
    applyOutcome(book.id, outcome);
  }

  /** OCR 批次：环形取 ≤BATCH_SIZE 未完成页 → 离屏渲染 → parse_pdf → 排翻译链 */
  async function processOcrBatch(book: PickTarget): Promise<void> {
    const lib = useLibraryStore();
    const take = ringCollect(book, (page) => (needsOcr(page) ? page : null));
    if (take.length === 0) return; // 竞态：已全部完成
    pushLlmLog(`[ui] OCR 批次 p${take.map((p) => p.index).join(",")}（${book.name}）`);

    // 离屏渲染：路径优先用 Rust 解析过的（聚焦书 currentPdf.pdfPath），
    // 后台书按物理命名 name-id 拼装（与 load_pdf 同构）
    const pdfPath =
      (book.focused ? lib.currentPdf?.pdfPath : undefined) ??
      `${lib.repoRoot}/${book.name}-${book.id}.pdf`;
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
        pushLlmLog(`[ui] 翻译批次失败 p${pages.join(",")}: ${String(err)}`);
        return true;
      });
    translateChains.set(bookId, next);
    void next.then((noProgress) => {
      if (translateChains.get(bookId) !== next) return;
      translateChains.delete(bookId);
      // 仍有未翻译页（失败/漏批）→ 唤醒重试支路（strike 机制防打转）；
      // 后台书无缓存，用 noProgress（整批无进展）兜底
      const cached = useLibraryStore().pdfs[bookId];
      if (noProgress || cached?.pages.some(needsTranslation)) wake();
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
    modelsBusy,
    parsing,
    standing,
    wake,
    checkEnv,
    installEnv,
    downloadModels,
    startService,
    stopService,
    autoStartIfEnabled,
  };
});
