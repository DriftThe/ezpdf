import { defineStore } from "pinia";
import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import type { LlmVerifyReport } from "../../src-tauri/bindings/LlmVerifyReport";
import type { UpdateInfo } from "../../src-tauri/bindings/UpdateInfo";
import type { PiModel, PiProvider } from "../lib/piModels.generated";
import {
  CUSTOM_PROVIDER,
  EMPTY_PRESET,
  type LlmPresetCompat,
  SUPPORTED_API,
  detectProviderId,
  findModel,
  findProvider,
  loadCatalog,
  presetCompat,
  sortProviders,
  usableModels,
} from "../lib/piModels";
import { toast } from "../composables/toast";
import { useLibraryStore } from "./library";
import { BLOCK_TYPE_OPTIONS, DEFAULT_TRANSLATED_TYPES } from "../lib/blocks";
import {
  currentLocale,
  defaultTargetLang,
  detectLocale,
  isAppLocale,
  setLocale,
  t,
} from "../lib/i18n";
import type { AppLocale } from "../locales";

/** 非 Tauri 环境（纯浏览器 pnpm dev）：invoke 必败，持久化整体静默 */
const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export interface LlmSettings {
  /**
   * 供应商预设（用户 2026-09-15 整合 pi-ai）：pi-ai 目录的供应商 id，或 "custom"。
   * 预设只负责「识别 + 填端点 + 协议/关思考参数适配」，网络请求仍是 Rust 的
   * OpenAI 兼容客户端（见 lib/piModels.ts）。
   */
  provider: string;
  /** 预设模型派生的兼容性快照（选供应商/模型时重算；自定义 = 全空） */
  preset: LlmPresetCompat;
  baseUrl: string;
  apiKey: string; // 阶段1起改存系统凭据库（keyring）
  model: string;
  targetLang: string;
  /** 智能上下文翻译：跨页截断文本经 agent loop 请求上下文联合翻译（默认开） */
  smartContext: boolean;
  /**
   * 关思考请求参数策略（用户 2026-09-14）："auto" 按预设/端点推断；
   * "reasoning" | "enable_thinking" | "thinking_type" 显式；"none" 不加参数。
   * 由「验证」按钮探测成功后回写；预设已给出关思考形态时无需探测。
   */
  thinkingOff: string;
}

/** invoke 传给 Rust 的扁平 LLM 配置（translate.rs LlmConfig，serde camelCase） */
export interface LlmInvokePayload {
  baseUrl: string;
  apiKey: string;
  model: string;
  targetLang: string;
  smartContext: boolean;
  thinkingOff: string;
  provider: string;
  api: string;
  thinkingFormat: string;
  thinkingOffKind: string;
  thinkingOffValue: string | null;
  maxTokensField: string;
  modelReasoning: boolean | null;
  extraHeaders: Record<string, string>;
  /** 送翻块类型（Rust LlmConfig.translateTypes；空数组 = 用后端内置默认） */
  translateTypes: string[];
}

/**
 * 常规开关（用户 2026-09-14：解析页删除，两个启动开关都归常规）：
 * - autoLaunch：启动时自动唤醒 OCR 服务（环境+模型全就绪才拉起）
 * - resumeOnStart：启动时自动续跑未完成的解析；关闭则启动后为暂停态
 *   （工具栏显示「启动翻译」）；两者都开才会自动进入运行态
 * - theme：界面主题（浅色/深色/跟随系统；标题栏右侧切换）
 */
export type ThemeMode = "system" | "light" | "dark";

export interface GeneralSettings {
  autoLaunch: boolean;
  resumeOnStart: boolean;
  theme: ThemeMode;
  /** 界面语言（用户 2026-09-15）：简中 / 繁中 / English；首启按系统语言定初值 */
  lang: AppLocale;
  /**
   * 参与翻译的块类型（用户 2026-09-15 可配，见 lib/blocks.ts BLOCK_TYPE_OPTIONS）。
   * 影响面：只决定「送翻 + 覆盖渲染」的类型集合，未勾选类型原 PDF 像素直出；
   * 已翻译的页面不会重翻（要重来请用仓库行的「清除解析状态」）。
   */
  translateTypes: string[];
}

/** OCR 服务安装选项（用户 2026-09-14）：一键安装服务时是否走国内镜像源 */
export interface OcrSettings {
  installMirror: boolean;
}

/**
 * 设置页左侧导航子项。
 * 新增设置块三步：① 此处扩展 union（如 "theme"）
 * ② 新建 src/components/settings/sections/ThemeSection.vue（抄现有 section 当模板，
 *    根元素 class="set-pane"；表单状态在本 store 加 ref）
 * ③ 在 SettingsPage.vue 的 SECTIONS 注册表加一行 —— union 扩了不注册会编译报错
 */
export type SettingsSection = "llm" | "ocr" | "common";

/**
 * auth.cfg 临时读取（用户拍板 2026-09-14）：LLM 四配置放项目根 auth.cfg（gitignored），
 * 此处解析后填充 settings.llm。仅 dev 生效——避免 `pnpm tauri build` 把密钥烤进 bundle；
 * 文件缺失/格式异常一律静默（fresh clone 不能因此挂）。正式方案 = settings.json + keyring。
 */
function parseAuthCfg(text: string): Partial<LlmSettings> {
  const out: Partial<LlmSettings> = {};
  for (const line of text.split(/\r?\n/)) {
    const m = /^\s*(baseUrl|apiKey|model|targetLang)\s*:\s*"([^"]*)"\s*,?\s*$/.exec(line);
    if (m) (out as Record<string, string>)[m[1]] = m[2];
  }
  return out;
}

async function fillLlmFromAuthCfg(llm: { value: LlmSettings }): Promise<void> {
  if (!import.meta.env.DEV) return; // 生产构建死代码消除：auth.cfg 不会进 bundle
  const files = import.meta.glob("../../auth.cfg", { query: "?raw", import: "default" }) as Record<
    string,
    () => Promise<string>
  >;
  const loader = files["../../auth.cfg"];
  if (!loader) return;
  try {
    const parsed = parseAuthCfg(await loader());
    llm.value = { ...llm.value, ...parsed };
  } catch {
    /* auth.cfg 缺失/不可读：保持空配置（翻译整体跳过） */
  }
}

/** config.json 磁盘结构（save_settings 整份写、load_settings 整份读） */
interface PersistedConfig {
  llm?: Partial<LlmSettings>;
  general?: Partial<GeneralSettings>;
  ocr?: Partial<OcrSettings> & { autoLaunch?: unknown };
  /** 上次打开的仓库根目录（启动时自动打开；空 = 未选择） */
  repo?: string | null;
  /** 旧版分节（仅迁移用，保存时不再写出） */
  parse?: { resumeOnStart?: unknown };
}

/** 逐段浅合并（未知字段/类型不符一律忽略，坏配置不能把设置页打挂） */
function isRecord(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === "object" && !Array.isArray(v);
}
function mergeSection<T extends object>(target: T, patch: unknown): void {
  if (!isRecord(patch)) return;
  for (const key of Object.keys(target) as Array<keyof T>) {
    const v = patch[key as string];
    if (typeof v === typeof target[key]) target[key] = v as T[keyof T];
  }
}

/** preset 是嵌套对象（浅合并会整份替换），逐字段校验后再落地 */
function mergePreset(target: LlmPresetCompat, patch: unknown): void {
  if (!isRecord(patch)) return;
  for (const key of Object.keys(target) as Array<keyof LlmPresetCompat>) {
    const v = patch[key as string];
    if (key === "extraHeaders") {
      if (isRecord(v)) {
        const headers: Record<string, string> = {};
        for (const [hk, hv] of Object.entries(v)) if (typeof hv === "string") headers[hk] = hv;
        target.extraHeaders = headers;
      }
      continue;
    }
    // 可空字段默认值是 null，不能用 typeof 比对（typeof null === "object"），按实际类型校验
    if (key === "thinkingOffValue") {
      if (v === null || typeof v === "string") target.thinkingOffValue = v;
      continue;
    }
    if (key === "reasoning") {
      if (v === null || typeof v === "boolean") target.reasoning = v;
      continue;
    }
    if (typeof v === typeof target[key]) target[key] = v as never;
  }
}

export const useSettingsStore = defineStore("settings", () => {
  /** 设置整页是否打开（覆盖 sidebar + reader 视窗，保留顶部工具栏；不卸载原视窗） */
  const pageOpen = ref(false);
  /** 当前选中的设置子项 */
  const section = ref<SettingsSection>("llm");

  const llm = ref<LlmSettings>({
    provider: CUSTOM_PROVIDER,
    preset: { ...EMPTY_PRESET, extraHeaders: {} },
    baseUrl: "",
    apiKey: "",
    model: "",
    targetLang: "",
    smartContext: true,
    thinkingOff: "auto",
  });

  /** pi-ai 目录（懒加载；null = 未加载——纯浏览器模式不加载） */
  const catalog = ref<PiProvider[] | null>(null);
  /** 模型搜索词（设置页 UI 状态） */
  const modelQuery = ref("");

  /** 目录加载（幂等；非 Tauri/加载失败都静默：预设不可用时手填 Base URL 仍能跑） */
  let catalogPromise: Promise<PiProvider[] | null> | null = null;
  function ensureCatalog(): Promise<PiProvider[] | null> {
    catalogPromise ??= loadCatalog()
      .then((list) => {
        catalog.value = list;
        return list;
      })
      .catch((e) => {
        console.warn("[settings] pi-ai 目录加载失败:", e);
        return null;
      });
    return catalogPromise;
  }

  /** 可用（OpenAI 兼容）预设供应商：有至少一个模型能走当前客户端 */
  const usableProviders = computed<PiProvider[]>(() =>
    sortProviders((catalog.value ?? []).filter((p) => usableModels(p).length > 0)),
  );
  /** 协议暂不支持（仅可识别）的预设供应商 */
  const otherProviders = computed<PiProvider[]>(() =>
    sortProviders((catalog.value ?? []).filter((p) => usableModels(p).length === 0)),
  );
  /** 当前预设供应商（自定义 = null） */
  const presetProvider = computed<PiProvider | null>(() =>
    findProvider(catalog.value ?? [], llm.value.provider),
  );
  /** 当前预设供应商的可用模型（按搜索词过滤） */
  const presetModels = computed<PiModel[]>(() => {
    const provider = presetProvider.value;
    if (!provider) return [];
    const q = modelQuery.value.trim().toLowerCase();
    return provider.models.filter(
      (m) => !q || m.id.toLowerCase().includes(q) || m.name.toLowerCase().includes(q),
    );
  });

  /** LLM 验证/模型拉取进行中（按钮防重入 + 文案） */
  const verifying = ref(false);
  const modelsFetching = ref(false);
  /** /models 拉取结果（模型输入框 datalist 补全） */
  const modelOptions = ref<string[]>([]);

  /** 当前应用版本 + 更新检查状态（常规设置页展示；启动静默检查一次） */
  const appVersion = ref("");
  const updateBusy = ref(false);
  const updateText = ref(t("update.notChecked"));

  const general = ref<GeneralSettings>({
    autoLaunch: true,
    resumeOnStart: true,
    theme: "system",
    lang: currentLocale(),
    translateTypes: [...DEFAULT_TRANSLATED_TYPES],
  });

  const ocr = ref<OcrSettings>({
    installMirror: true,
  });

  /** 上次选择的仓库根目录（config.json 持久化；library 启动时据此自动打开） */
  const repoPath = ref<string | null>(null);

  /**
   * 读取持久化设置（config.json；dev = 仓库根、生产 = 应用目录）：
   * auth.cfg（dev）先落地，持久化配置覆盖其上——应用内改过的值优先于开发默认。
   * 单次幂等（共享 promise），App 启动与设置页打开都安全。
   */
  let loadPromise: Promise<void> | null = null;
  /**
   * 成功读过盘才允许写回（否则用户的 config.json 会被内存里的默认值覆盖）。
   * 场景：设置页返回键/主题/语言/仓库路径都是"整份写"，加载失败时写盘 = 清空用户配置。
   */
  let loaded = false;
  function ensureLoaded(): Promise<void> {
    loadPromise ??= (async () => {
      if (!isTauri) return;
      try {
        appVersion.value = await getVersion();
      } catch {
        /* 版本号拿不到不致命（更新检查时仍会回填） */
      }
      try {
        const text = await invoke<string | null>("load_settings");
        // 无配置文件（首次启动）：不能在这里 return——下面的语言/目标语言默认值还要补
        if (text) {
          const cfg = JSON.parse(text) as PersistedConfig;
          // preset 是嵌套对象：浅合并会整份替换，单独逐字段校验
          const patch: Record<string, unknown> = isRecord(cfg.llm) ? { ...cfg.llm } : {};
          delete patch.preset;
          mergeSection(llm.value, patch);
          mergePreset(llm.value.preset, cfg.llm?.preset);
          // 旧版迁移：ocr.autoLaunch / parse.resumeOnStart → general（先落旧值，新节覆盖）
          if (typeof cfg.ocr?.autoLaunch === "boolean") general.value.autoLaunch = cfg.ocr.autoLaunch;
          if (typeof cfg.parse?.resumeOnStart === "boolean") general.value.resumeOnStart = cfg.parse.resumeOnStart;
          mergeSection(general.value, cfg.general);
          mergeSection(ocr.value, cfg.ocr); // installMirror（autoLaunch 键不在目标对象上，被忽略）
          if (typeof cfg.repo === "string" && cfg.repo.trim()) repoPath.value = cfg.repo;
        }
      } catch (e) {
        console.warn("[settings] config.json 读取失败，使用默认值:", e);
        return; // 读失败：保持 loaded=false，本次会话拒绝写盘（不覆盖用户配置）
      }
      loaded = true;
      // 界面语言：配置值非法（手改坏/旧字段）回落系统检测；配置优先于 localStorage 镜像
      if (!isAppLocale(general.value.lang)) general.value.lang = detectLocale();
      setLocale(general.value.lang);
      // 目标语言初值（用户 2026-09-15：跟随系统语言；仅从未设置过时填，auth.cfg/旧配置优先）
      if (!llm.value.targetLang.trim()) llm.value.targetLang = defaultTargetLang(general.value.lang);
      // 送翻类型：过滤非法/过时标签；空集合视为未设置 → 回落内置默认（Rust 侧同样语义）
      const picked = Array.isArray(general.value.translateTypes) ? general.value.translateTypes : [];
      const valid = picked.filter((t) => typeof t === "string" && BLOCK_TYPE_OPTIONS.includes(t));
      general.value.translateTypes = valid.length ? valid : [...DEFAULT_TRANSLATED_TYPES];
      // 供应商预设迁移/补全（旧配置只有 baseUrl+model）：反查目录 → 重算兼容快照
      await ensureCatalog();
      if (
        llm.value.provider !== CUSTOM_PROVIDER &&
        !findProvider(catalog.value ?? [], llm.value.provider)
      ) {
        llm.value.provider = CUSTOM_PROVIDER; // 目录升级后供应商已不存在
      }
      if (llm.value.provider === CUSTOM_PROVIDER && llm.value.baseUrl.trim()) {
        llm.value.provider = detectProviderId(catalog.value ?? [], llm.value.baseUrl);
      }
      syncPreset();
    })();
    return loadPromise;
  }

  async function fillDefaults(): Promise<void> {
    await fillLlmFromAuthCfg(llm);
    await ensureLoaded();
  }
  void fillDefaults();

  /** 由当前「供应商 + 模型」重算兼容快照（目录未加载/自定义/模型不在目录 → 空快照） */
  function syncPreset(): void {
    llm.value.preset = presetCompat(
      findModel(catalog.value ?? [], llm.value.provider, llm.value.model),
    );
  }

  /** 手填模型后重算快照（设置页输入框 change 事件） */
  function refreshPreset(): void {
    syncPreset();
  }

  /** 选预设供应商：填端点；模型若不属于该供应商则自动取第一个可用模型 */
  function applyProvider(id: string): void {
    llm.value.provider = id;
    if (id === CUSTOM_PROVIDER) {
      syncPreset();
      return;
    }
    const provider = findProvider(catalog.value ?? [], id);
    if (!provider) return;
    if (!findModel(catalog.value ?? [], id, llm.value.model)) {
      const first = usableModels(provider)[0] ?? provider.models[0];
      if (first) llm.value.model = first.id;
    }
    const model = findModel(catalog.value ?? [], id, llm.value.model) ?? provider.models[0];
    if (model) llm.value.baseUrl = model.baseUrl;
    syncPreset();
  }

  /** 选预设模型：同步该模型端点（同一供应商可能有多个端点）+ 重算快照 */
  function applyModel(modelId: string): void {
    llm.value.model = modelId;
    const model = findModel(catalog.value ?? [], llm.value.provider, modelId);
    if (model) llm.value.baseUrl = model.baseUrl;
    syncPreset();
  }

  /** invoke 用的扁平配置（parse.ts 调度 + verify_llm 共用）；三要素缺失 = null（翻译整体跳过） */
  function llmInvokePayload(): LlmInvokePayload | null {
    const s = llm.value;
    if (!s.baseUrl.trim() || !s.apiKey.trim() || !s.model.trim()) return null;
    return {
      baseUrl: s.baseUrl.trim(),
      apiKey: s.apiKey,
      model: s.model.trim(),
      targetLang: s.targetLang,
      smartContext: s.smartContext,
      thinkingOff: s.thinkingOff,
      provider: s.provider,
      api: s.preset.api,
      thinkingFormat: s.preset.thinkingFormat,
      thinkingOffKind: s.preset.thinkingOffKind,
      thinkingOffValue: s.preset.thinkingOffValue,
      maxTokensField: s.preset.maxTokensField,
      modelReasoning: s.preset.reasoning,
      extraHeaders: s.preset.extraHeaders,
      translateTypes: [...general.value.translateTypes],
    };
  }

  /** 供应商是不是「可用预设」（OpenAI 兼容）：UI 显示兼容性提示用 */
  function presetUsable(): boolean {
    return !llm.value.preset.api || llm.value.preset.api === SUPPORTED_API;
  }

  function openPage(): void {
    pageOpen.value = true;
    // sidebar 收起时工具栏满宽，会盖住设置页左列顶部的返回键——打开设置先复位展开
    useLibraryStore().sidebarOpen = true;
  }

  /** 整份写盘（save 与仓库路径即时持久化共用） */
  async function doSave(): Promise<void> {
    if (!loaded) {
      console.error("[settings] save skipped: the config was never loaded successfully");
      return;
    }
    const payload: PersistedConfig = {
      llm: { ...llm.value },
      general: { ...general.value },
      ocr: { ...ocr.value },
      repo: repoPath.value,
    };
    await invoke("save_settings", { json: JSON.stringify(payload, null, 2) });
  }

  /** 保存并退出（设置页返回键）：失败留在设置页报错；成功不再 toast（用户 2026-09-14） */
  async function save(): Promise<void> {
    if (!isTauri) {
      pageOpen.value = false;
      return;
    }
    if (!loaded) {
      // 读盘都没成功：写回只会清空用户配置，留在设置页并说明原因
      toast(t("settings.saveNotLoaded"), "error");
      return;
    }
    try {
      await doSave();
      pageOpen.value = false;
    } catch (e) {
      toast(t("settings.saveFailed", { error: String(e) }), "error");
    }
  }

  /** 仓库选择变化即时持久化（不弹 toast；失败仅控制台告警，不阻塞打开仓库） */
  async function setRepoPath(path: string | null): Promise<void> {
    repoPath.value = path;
    if (!isTauri) return;
    try {
      await doSave();
    } catch (e) {
      console.warn("[settings] 仓库路径持久化失败:", e);
    }
  }

  /** 主题切换即时持久化（用户 2026-09-14）：无需等设置页退出保存；localStorage 镜像由 theme.ts 维护 */
  async function setTheme(mode: ThemeMode): Promise<void> {
    general.value.theme = mode;
    if (!isTauri) return;
    try {
      await doSave();
    } catch (e) {
      console.warn("[settings] 主题持久化失败:", e);
    }
  }

  /** 界面语言切换（用户 2026-09-15）：即时生效 + 即时持久化（localStorage 镜像由 i18n.ts 维护） */
  async function setLang(locale: AppLocale): Promise<void> {
    general.value.lang = locale;
    setLocale(locale);
    if (!isTauri) return;
    try {
      await doSave();
    } catch (e) {
      console.warn("[settings] 界面语言持久化失败:", e);
    }
  }

  /** 验证 LLM（用户 2026-09-14）：连通性 + 关思考策略探测；策略回写 thinkingOff */
  async function verifyLlm(): Promise<void> {
    if (verifying.value) return;
    const payload = llmInvokePayload();
    if (!payload) {
      toast(t("llm.fillAllRequired"), "warn");
      return;
    }
    verifying.value = true;
    try {
      const report = await invoke<LlmVerifyReport>("verify_llm", { llm: payload });
      llm.value.thinkingOff = report.strategy;
      // 找不到关思考参数时明确警告；预设标记为非思考模型则不警告（用户 2026-09-15）
      const warn = report.strategy === "none" && !report.presetNoThinking;
      toast(report.message, warn ? "warn" : "info");
    } catch (e) {
      toast(t("llm.verifyFailed", { error: String(e) }), "error");
    } finally {
      verifying.value = false;
    }
  }

  /** 拉取 /models 列表（失败 toast 兜底；成功不打扰） */
  async function fetchModels(silent = false): Promise<void> {
    if (modelsFetching.value) return;
    const { baseUrl, apiKey } = llm.value;
    if (!baseUrl.trim() || !apiKey.trim()) {
      if (!silent) toast(t("llm.fillBaseKey"), "warn");
      return;
    }
    modelsFetching.value = true;
    try {
      modelOptions.value = await invoke<string[]>("fetch_llm_models", { baseUrl, apiKey });
      if (!silent) toast(t("llm.modelsFetched", { count: modelOptions.value.length }));
    } catch (e) {
      toast(t("llm.fetchModelsFailed", { error: String(e) }), "error");
    } finally {
      modelsFetching.value = false;
    }
  }

  /** 检查更新（用户 2026-09-14）：GitHub Releases 最新版 vs 当前版；
   *  manual=false（启动）失败静默，manual=true（按钮）失败 toast */
  async function checkUpdate(manual = false): Promise<void> {
    if (!isTauri || updateBusy.value) return;
    updateBusy.value = true;
    try {
      const info = await invoke<UpdateInfo>("check_update");
      appVersion.value = info.current;
      if (info.newer && info.latest) {
        updateText.value = t("update.found", { latest: info.latest });
        toast(t("update.foundToast", { latest: info.latest, current: info.current }), "info");
      } else {
        updateText.value = t("update.upToDate", { current: info.current });
      }
    } catch (e) {
      updateText.value = t("update.failed");
      if (manual) toast(t("update.failedToast", { error: String(e) }), "warn");
    } finally {
      updateBusy.value = false;
    }
  }

  return {
    pageOpen,
    section,
    llm,
    general,
    ocr,
    repoPath,
    verifying,
    modelsFetching,
    modelOptions,
    appVersion,
    updateBusy,
    updateText,
    catalog,
    modelQuery,
    usableProviders,
    otherProviders,
    presetProvider,
    presetModels,
    openPage,
    save,
    setRepoPath,
    setTheme,
    setLang,
    ensureCatalog,
    applyProvider,
    applyModel,
    refreshPreset,
    presetUsable,
    llmInvokePayload,
    verifyLlm,
    fetchModels,
    checkUpdate,
    ensureLoaded,
  };

});
