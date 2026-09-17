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

/** 非 Tauri 环境（纯浏览器 pnpm dev）：invoke 必败，调度整体静默（同 listen().catch 哲学） */

/** 页级谓词（调度取样共用） */
const needsOcr = (p: PageInfo): boolean => !p.finished;
const needsTranslation = (p: PageInfo): boolean => p.finished && !p.translated;

/** 送翻类型：空列表 = 内置默认（与 Rust is_translatable 的回落一致） */
function effectiveTypes(): readonly string[] {
  const types = useSettingsStore().general.translateTypes;
  return types.length > 0 ? types : DEFAULT_TRANSLATED_TYPES;
}

/**
 * 表格补翻（用户 2026-09-16）：表格支持之前翻过的页面里，表块从未被送翻过
 * （translation=null），而"已翻页不回翻"让它们永远轮不到。
 * 只认**Rust 已补齐网格**（load_pdf 时解析落盘 = 可解析）且译文为空的表格；
 * 解析不出网格的表格不成为目标，其余块类型一律不碰。
 */
function needsTableBackfill(p: PageInfo): boolean {
  if (!p.finished || !effectiveTypes().includes("table")) return false;
  return p.blocks.some((b) => b.type === "table" && !b.translation && !!b.grid);
}

/** 翻译取样总谓词：未翻页 + 已翻页里待补翻的表格 */
const needsWork = (p: PageInfo): boolean => needsTranslation(p) || needsTableBackfill(p);

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

  /** 在线模式握手：读 /health 拿服务端公布的批大小（用户 2026-09-15）。
   *  每次 OCR 请求前都会调一次——服务端换负载/换配置后客户端立刻跟上；
   *  失败即抛错，让这一批按失败计连败（不要拿过期数字继续发） */
  async function handshakeOnline(): Promise<ParseServiceHealth> {
    const ocr = useSettingsStore().ocr;
    const health = await invoke<ParseServiceHealth>("ocr_health", {
      url: ocr.url,
      token: ocr.token.trim(),
    });
    onlineHealth.value = health;
    return health;
  }

  /** 健康报告 → 本地化文案（toast 用） */
  function healthDetail(h: ParseServiceHealth): string {
    return t("ocr.healthDetail", {
      pid: h.pid ?? "?",
      ms: h.elapsedMs,
      batch: h.maxBatchPages,
    });
  }

  /** 健康报告 → 英文一行摘要（日志用；Rust/Python 侧日志同样一律英文） */
  function healthLine(h: ParseServiceHealth): string {
    return `pid ${h.pid ?? "-"} ${h.elapsedMs}ms max_batch_pages=${h.maxBatchPages}`;
  }

  /** 在线模式连接（探活通过才登记为 OCR 目标）；失败抛错，调用方决定 toast 还是日志 */
  async function connectOnline(url: string): Promise<ParseServiceHealth> {
    const health = await invoke<ParseServiceHealth>("ocr_start_remote", {
      url,
      token: useSettingsStore().ocr.token.trim(),
    });
    onlineHealth.value = health;
    return health;
  }

  /** 启动服务（设置页按钮）：本地托管 → 拉起子进程；在线服务 → 探活后登记端点 */
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

  /** 在线模式地址探活（设置页地址框右侧「测试」按钮，用户 2026-09-15）：
   *  只探测健康度，不改连接状态——连不连是「启动服务」的事。
   *  顺便把服务端公布的批大小显示出来（地址下方 hint） */
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

  /** 启动自动唤醒（常规设置 autoLaunch，用户 2026-09-14）：环境/模型全就绪才拉起，
   *  缺件只记日志不打扰（到 OCR 服务设置页一键安装服务）。
   *  在线模式（2026-09-15）不查本地环境：基础环境也能连远端服务，探活通过即登记 */
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
  /** 本地托管模式的批大小（用户拍板 2026-09-14：一次 ≤4 页、同书，不足传剩余页）。
   *  在线模式不用它——批大小由服务端 /health 公布（用户 2026-09-15），见 PROTOCOL.md §4 */
  const LOCAL_BATCH_SIZE = 4;
  /** 最近一次在线握手结果（服务端公布的批大小 + pid/耗时，供设置页显示） */
  const onlineHealth = ref<ParseServiceHealth | null>(null);
  /** 当前该一批发几页：本地 = 客户端定；在线 = 服务端公布（没握到手时回落 4） */
  function currentBatchSize(): number {
    if (useSettingsStore().ocr.mode !== "online") return LOCAL_BATCH_SIZE;
    return onlineHealth.value?.maxBatchPages ?? LOCAL_BATCH_SIZE;
  }
  /** 首次 + 重试 2 次 = 3 连败 → 本轮停该类批次 */
  const MAX_ATTEMPTS = 3;
  /** 书级连败计数（OCR / 翻译分开：翻译持续失败只关翻译支路，OCR 照跑；反之亦然）；wake 时清零 */
  const ocrStrikes = new Map<string, number>();
  const translateStrikes = new Map<string, number>();

  /** OCR 批次的前提：解析服务可用（本地托管已连上 / 在线服务已登记） */
  function canOcr(): boolean {
    return serviceStatus.value === "connected";
  }

  /** 翻译批次的前提：仅需 LLM 配置——与解析服务无关（用户 2026-09-15 解耦）。
   *  关掉翻译时 payload 也非空（Rust 走"原文当译文"路径，无需密钥），
   *  所以基础环境（没装依赖/模型）也能把已 OCR 的页处理完 */
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
        standing.value = true; // 一轮扫完无事可做 → 挂起等事件
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
      // OCR 需要解析服务（在线/本地托管皆可）；服务不可用时跳过，等连上再唤醒
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
    if (current?.pages.some(needsWork)) {
      llmMissingNotified = true;
      pushLlmLog("[ui] LLM not configured (baseUrl/apiKey/model empty) → translation skipped; fill it in Settings");
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

  /** 环形收集 ≤limit 个命中页（pick 返回 null = 跳过该页；limit 默认本地批大小） */
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
    if (!llm) throw new Error("LLM not configured (baseUrl/apiKey/model empty)");
    const root = lib.repoRoot;
    if (!root) throw new Error("no repository open");
    const pages = ringCollect(book, (page) => (needsWork(page) ? page.index : null));
    if (pages.length === 0) return; // 竞态：已全部翻译
    pushLlmLog(`[ui] translation retry batch p${pages.join(",")} (${book.name})`);
    const outcome = await invokeTranslate(root, book.id, pages, llm);
    if (outcome.updatedPages.length === 0) {
      throw new Error("translation made no progress"); // 计入 strike，防止坏页空转
    }
  }

  /** OCR 批次：环形取 ≤批大小 未完成页 → 离屏渲染 → parse_pdf → 排翻译链。
   *  在线模式先握手 /health 拿服务端公布的批大小（用户 2026-09-15：批大小由服务端定，
   *  每次请求前重新协商；握手失败按批次失败计连败），本地托管沿用客户端自己的 4 页 */
  async function processOcrBatch(book: PickTarget): Promise<void> {
    const lib = useLibraryStore();
    if (useSettingsStore().ocr.mode === "online") await handshakeOnline();
    const limit = currentBatchSize();
    const take = ringCollect(book, (page) => (needsOcr(page) ? page : null), limit);
    if (take.length === 0) return; // 竞态：已全部完成
    pushLlmLog(`[ui] OCR batch p${take.map((p) => p.index).join(",")} (${book.name})`);

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
      // 仍有未翻译页（失败/漏批）→ 唤醒补翻支路（translateStrikes 防打转）；
      // 后台书无缓存，用 noProgress（整批无进展）兜底。自我续跑用 wake(false)：
      // 不重置连败预算，避免 LLM 坏掉时每批白试
      const cached = useLibraryStore().pdfs[bookId];
      if (noProgress || cached?.pages.some(needsWork)) wake(false);
    });
  }

  /** 调一次 translate_pdf 并把结果并回 store（翻译重试支路与翻译链共用同一份 invoke 参数） */
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
      await lib.loadPdf(id);
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
