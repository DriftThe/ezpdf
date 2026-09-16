import { defineStore } from "pinia";
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import type { LlmVerifyReport } from "../../src-tauri/bindings/LlmVerifyReport";
import type { UpdateInfo } from "../../src-tauri/bindings/UpdateInfo";
import type { PiProvider } from "../lib/piModels.generated";
import {
  CUSTOM_PROVIDER,
  DEFAULT_API,
  EMPTY_PRESET,
  type LlmPresetCompat,
  detectProviderId,
  findModel,
  findProvider,
  isSupportedApi,
  loadCatalog,
  presetCompat,
  sortProviders,
  usableModels,
} from "../lib/piModels";
import { toast } from "../composables/toast";
import { isTauri } from "../lib/env";
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

interface LlmSettings {
  /**
   * 是否启用翻译（用户 2026-09-15，设置→LLM 首项）：关闭时 OCR 出的文本直接以
   * content 作为译文落盘并标记完成（不请求 LLM，也不留 null），避免以后重新打开
   * 翻译时把老内容回翻。默认开。
   */
  translateEnabled: boolean;
  /**
   * 供应商预设（用户 2026-09-15 整合 pi-ai）：pi-ai 目录的供应商 id，或 "custom"。
   * 预设只负责「识别 + 填端点 + 协议/关思考参数适配」，网络请求仍是 Rust 客户端
   * （三种线协议，见 lib/piModels.ts 与 docs/protocols.md）。
   */
  provider: string;
  /**
   * 兼容性快照：预设模型由目录派生（选供应商/模型时重算）；自定义端点由用户选协议
   * （preset.api），其余字段留空 → 关思考靠验证按钮探测。
   */
  preset: LlmPresetCompat;
  baseUrl: string;
  apiKey: string; // 明文存在 config.json（README「数据存放位置」有说明）
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
  api: string;
  thinkingOffKind: string;
  thinkingOffValue: string | null;
  maxTokensField: string;
  modelReasoning: boolean | null;
  extraHeaders: Record<string, string>;
  /** 送翻块类型（Rust LlmConfig.translateTypes；空数组 = 用后端内置默认） */
  translateTypes: string[];
  /** 是否启用翻译（Rust LlmConfig.translateEnabled；false = 原文当译文落盘） */
  translateEnabled: boolean;
}

/**
 * 常规开关（用户 2026-09-14：解析页删除，两个启动开关都归常规）：
 * - autoLaunch：启动时自动唤醒 OCR 服务（环境+模型全就绪才拉起）
 * - resumeOnStart：启动时自动续跑未完成的解析；关闭则启动后为暂停态
 *   （工具栏显示「启动翻译」）；两者都开才会自动进入运行态
 * 两者默认关闭（用户 2026-09-15 核对：首启即暂停、不主动拉起服务，与 README
 * 「出于省电考虑，启动后为暂停状态」一致）——
 * 已存在的配置照旧生效，改默认值只影响首次启动（无 config.json）。
 * - theme：界面主题（浅色/深色/跟随系统；标题栏右侧切换）
 */
export type ThemeMode = "system" | "light" | "dark";

interface GeneralSettings {
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

/**
 * OCR 解析服务来源（用户 2026-09-15）：
 * - local：应用托管的 pyserver（spawn + 就绪握手 + stdin EOF 收尾，需装依赖/模型）；
 * - online：远端解析服务（同款 HTTP 协议，见 pyserver/PROTOCOL.md），只需一个可达
 *   的 URL——基础环境（没装 torch/模型）也能用；翻译不依赖它（Rust 直连 LLM）。
 */
type OcrMode = "local" | "online";

/** OCR 服务设置：安装镜像源 + 服务来源（本地托管 / 在线服务） */
interface OcrSettings {
  installMirror: boolean;
  mode: OcrMode;
  /** 在线模式的解析服务地址（如 http://127.0.0.1:9055）；local 模式忽略 */
  url: string;
  /** 在线模式的服务令牌（部署方 token.txt 里那串）；服务端没开鉴权就留空 */
  token: string;
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
  ocr?: Partial<OcrSettings>;
  /** 上次打开的仓库根目录（启动时自动打开；空 = 未选择） */
  repo?: string | null;
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
    translateEnabled: true,
  });

  /** pi-ai 目录（懒加载；null = 未加载——纯浏览器模式不加载） */
  const catalog = ref<PiProvider[] | null>(null);

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
    autoLaunch: false,
    resumeOnStart: false,
    theme: "system",
    lang: currentLocale(),
    translateTypes: [...DEFAULT_TRANSLATED_TYPES],
  });

  const ocr = ref<OcrSettings>({
    installMirror: true,
    mode: "local",
    url: "",
    token: "",
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
  /** 读盘落地后的界面语言（写盘前用它兜住"被默认值顶掉"的语言，见 doSave） */
  let loadedLang: AppLocale | null = null;
  /** 用户在设置里显式改过界面语言（只有这种情况允许把新语言写进配置） */
  let langTouched = false;
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
          mergeSection(general.value, cfg.general);
          mergeSection(ocr.value, cfg.ocr);
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
      loadedLang = general.value.lang;
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
      autosaveArmed = true; // 到这一步内存里的值都来自磁盘，自动保存可以开始了
    })();
    return loadPromise;
  }

  async function fillDefaults(): Promise<void> {
    await fillLlmFromAuthCfg(llm);
    await ensureLoaded();
  }
  void fillDefaults();

  /**
   * 由当前「供应商 + 模型」重算兼容快照（目录未加载/模型不在目录 → 空快照）。
   * 自定义端点保留用户手选的协议——否则每次改模型名都会把协议清成空（= 退回 Chat
   * Completions），用户明明选着 Messages 却发出 chat 请求。
   */
  function syncPreset(): void {
    const preset = presetCompat(
      findModel(catalog.value ?? [], llm.value.provider, llm.value.model),
    );
    if (llm.value.provider === CUSTOM_PROVIDER) {
      preset.api = isSupportedApi(llm.value.preset.api) ? llm.value.preset.api : DEFAULT_API;
    }
    llm.value.preset = preset;
  }

  /** 手填模型后重算快照（设置页输入框 change 事件） */
  function applyModelInput(): void {
    syncPreset();
  }

  /** 当前生效协议：预设来自目录、自定义来自用户选择；空 = Chat Completions */
  const protocol = computed<string>(() =>
    isSupportedApi(llm.value.preset.api) ? llm.value.preset.api : DEFAULT_API,
  );

  /** 手选协议（用户 2026-09-16）：只有自定义端点能改，预设的协议由目录决定 */
  function applyProtocol(api: string): void {
    if (llm.value.provider !== CUSTOM_PROVIDER || !isSupportedApi(api)) return;
    llm.value.preset = { ...llm.value.preset, api };
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

  /** invoke 用的扁平配置（parse.ts 调度 + verify_llm 共用）；三要素缺失 = null（翻译整体跳过） */
  function llmInvokePayload(): LlmInvokePayload | null {
    const s = llm.value;
    // 关掉翻译时也要下发：Rust 收到的 translateEnabled=false 会把原文复制成译文
    // 并标记完成（不走网络），端点/密钥可以为空——基础环境照样能"处理完" OCR 文本
    if (s.translateEnabled && (!s.baseUrl.trim() || !s.apiKey.trim() || !s.model.trim())) {
      return null;
    }
    return {
      baseUrl: s.baseUrl.trim(),
      apiKey: s.apiKey,
      model: s.model.trim(),
      targetLang: s.targetLang,
      smartContext: s.smartContext,
      thinkingOff: s.thinkingOff,
      api: s.preset.api,
      thinkingOffKind: s.preset.thinkingOffKind,
      thinkingOffValue: s.preset.thinkingOffValue,
      maxTokensField: s.preset.maxTokensField,
      modelReasoning: s.preset.reasoning,
      extraHeaders: s.preset.extraHeaders,
      translateTypes: [...general.value.translateTypes],
      translateEnabled: s.translateEnabled,
    };
  }

  function openPage(): void {
    pageOpen.value = true;
    // sidebar 收起时工具栏满宽，会盖住设置页左列顶部的返回键——打开设置先复位展开
    useLibraryStore().sidebarOpen = true;
  }

  /**
   * 改动即自动落盘（用户 2026-09-17）：任何设置改动 **2 秒**后写一次 config.json，
   * 不必等退出设置页——写盘本来就是整份写，所以只是把时机提前（返回键仍会立即写一次）。
   * 两个前提：非 Tauri 环境不写；读盘成功前不写（否则内存里的默认值会覆盖用户配置，
   * 与 `loaded` 那道闸同一个理由）。
   */
  const AUTOSAVE_DEBOUNCE_MS = 2000;
  let autosaveTimer: ReturnType<typeof setTimeout> | null = null;
  let autosaveArmed = false;

  function cancelAutosave(): void {
    if (autosaveTimer !== null) {
      clearTimeout(autosaveTimer);
      autosaveTimer = null;
    }
  }

  function scheduleAutosave(): void {
    if (!isTauri || !autosaveArmed) return;
    cancelAutosave();
    autosaveTimer = setTimeout(() => {
      autosaveTimer = null;
      void (async () => {
        try {
          await doSave();
        } catch (e) {
          console.warn("[settings] 自动保存失败:", e); // 不打扰：下一次改动/退出时还会写
        }
      })();
    }, AUTOSAVE_DEBOUNCE_MS);
  }

  // 同步 flush：读盘期间的赋值（auth.cfg 填充、默认值补全、预设迁移）不能触发写盘
  watch([llm, general, ocr, repoPath], scheduleAutosave, { deep: true, flush: "sync" });

  /** 整份写盘（自动保存 / 返回键 / 即时持久化共用） */
  async function doSave(): Promise<void> {
    cancelAutosave(); // 这次写入取代等待中的自动保存
    // 界面语言只允许由 setLang 改动：写盘前若发现它偏离读盘值（且用户没动过设置），
    // 说明被某个默认值流程顶掉了——按读盘值写回，避免把系统语言覆盖进用户配置
    // （2026-09-15 真的发生过：一次启动把 en 写成 zh-CN，来源未复现）。
    if (!langTouched && loadedLang !== null && general.value.lang !== loadedLang) {
      console.warn(`[settings] lang reverted to ${general.value.lang}, keeping ${loadedLang}`);
      general.value.lang = loadedLang;
    }
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

  /** 立即生效的设置的即时持久化：不弹 toast，失败只控制台告警（不阻塞当前操作） */
  async function persistNow(what: string): Promise<void> {
    if (!isTauri) return;
    try {
      await doSave();
    } catch (e) {
      console.warn(`[settings] ${what}持久化失败:`, e);
    }
  }

  /** 仓库选择变化即时持久化（失败不阻塞打开仓库） */
  async function setRepoPath(path: string | null): Promise<void> {
    repoPath.value = path;
    await persistNow("仓库路径");
  }

  /** 主题切换即时持久化（用户 2026-09-14）：无需等设置页退出保存；localStorage 镜像由 theme.ts 维护 */
  async function setTheme(mode: ThemeMode): Promise<void> {
    general.value.theme = mode;
    await persistNow("主题");
  }

  /** 界面语言切换（用户 2026-09-15）：即时生效 + 即时持久化（localStorage 镜像由 i18n.ts 维护） */
  async function setLang(locale: AppLocale): Promise<void> {
    general.value.lang = locale;
    langTouched = true;
    setLocale(locale);
    await persistNow("界面语言");
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
      // 探测到的响应里仍带思考内容才警告（strategy "none" 也可能是"这个端点不需要参数"）；
      // 预设标记为非思考模型则不警告（用户 2026-09-15）
      const warn = report.thinkingOn && !report.presetNoThinking;
      toast(report.message, warn ? "warn" : "info");
    } catch (e) {
      toast(t("llm.verifyFailed", { error: String(e) }), "error");
    } finally {
      verifying.value = false;
    }
  }

  /** 拉取 /models 列表（失败 toast 兜底；成功不打扰） */
  async function fetchModels(): Promise<void> {
    if (modelsFetching.value) return;
    const { baseUrl, apiKey } = llm.value;
    if (!baseUrl.trim() || !apiKey.trim()) {
      toast(t("llm.fillBaseKey"), "warn");
      return;
    }
    modelsFetching.value = true;
    try {
      modelOptions.value = await invoke<string[]>("fetch_llm_models", {
        baseUrl,
        apiKey,
        api: protocol.value,
      });
      toast(t("llm.modelsFetched", { count: modelOptions.value.length }));
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
    usableProviders,
    otherProviders,
    presetProvider,
    protocol,
    openPage,
    save,
    setRepoPath,
    setTheme,
    setLang,
    ensureCatalog,
    applyProvider,
    applyProtocol,
    applyModelInput,
    llmInvokePayload,
    verifyLlm,
    fetchModels,
    checkUpdate,
    ensureLoaded,
  };

});
