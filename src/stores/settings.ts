import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import type { LlmVerifyReport } from "../../src-tauri/bindings/LlmVerifyReport";
import type { UpdateInfo } from "../../src-tauri/bindings/UpdateInfo";
import { toast } from "../composables/toast";
import { useLibraryStore } from "./library";

/** 非 Tauri 环境（纯浏览器 pnpm dev）：invoke 必败，持久化整体静默 */
const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export interface LlmSettings {
  baseUrl: string;
  apiKey: string; // 阶段1起改存系统凭据库（keyring）
  model: string;
  targetLang: string;
  /** 智能上下文翻译：跨页截断文本经 agent loop 请求上下文联合翻译（默认开） */
  smartContext: boolean;
  /**
   * 关思考请求参数策略（用户 2026-09-14）："auto" 按端点 URL 推断；
   * "reasoning" | "enable_thinking" | "thinking_type" 显式；"none" 不加参数。
   * 由「验证」按钮探测成功后回写。
   */
  thinkingOff: string;
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

export const useSettingsStore = defineStore("settings", () => {
  /** 设置整页是否打开（覆盖 sidebar + reader 视窗，保留顶部工具栏；不卸载原视窗） */
  const pageOpen = ref(false);
  /** 当前选中的设置子项 */
  const section = ref<SettingsSection>("llm");

  const llm = ref<LlmSettings>({
    baseUrl: "",
    apiKey: "",
    model: "",
    targetLang: "",
    smartContext: true,
    thinkingOff: "auto",
  });

  /** LLM 验证/模型拉取进行中（按钮防重入 + 文案） */
  const verifying = ref(false);
  const modelsFetching = ref(false);
  /** /models 拉取结果（模型输入框 datalist 补全） */
  const modelOptions = ref<string[]>([]);

  /** 当前应用版本 + 更新检查状态（常规设置页展示；启动静默检查一次） */
  const appVersion = ref("");
  const updateBusy = ref(false);
  const updateText = ref("未检查");

  const general = ref<GeneralSettings>({
    autoLaunch: true,
    resumeOnStart: true,
    theme: "system",
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
        if (!text) return;
        const cfg = JSON.parse(text) as PersistedConfig;
        mergeSection(llm.value, cfg.llm);
        // 旧版迁移：ocr.autoLaunch / parse.resumeOnStart → general（先落旧值，新节覆盖）
        if (typeof cfg.ocr?.autoLaunch === "boolean") general.value.autoLaunch = cfg.ocr.autoLaunch;
        if (typeof cfg.parse?.resumeOnStart === "boolean") general.value.resumeOnStart = cfg.parse.resumeOnStart;
        mergeSection(general.value, cfg.general);
        mergeSection(ocr.value, cfg.ocr); // installMirror（autoLaunch 键不在目标对象上，被忽略）
        if (typeof cfg.repo === "string" && cfg.repo.trim()) repoPath.value = cfg.repo;
      } catch (e) {
        console.warn("[settings] config.json 读取失败，使用默认值:", e);
      }
    })();
    return loadPromise;
  }

  async function fillDefaults(): Promise<void> {
    await fillLlmFromAuthCfg(llm);
    await ensureLoaded();
  }
  void fillDefaults();

  function openPage(): void {
    pageOpen.value = true;
    // sidebar 收起时工具栏满宽，会盖住设置页左列顶部的返回键——打开设置先复位展开
    useLibraryStore().sidebarOpen = true;
  }

  /** 整份写盘（save 与仓库路径即时持久化共用） */
  async function doSave(): Promise<void> {
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
    try {
      await doSave();
      pageOpen.value = false;
    } catch (e) {
      toast(`设置保存失败：${String(e)}`, "error");
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

  /** 验证 LLM（用户 2026-09-14）：连通性 + 关思考策略探测；策略回写 thinkingOff */
  async function verifyLlm(): Promise<void> {
    if (verifying.value) return;
    const { baseUrl, apiKey, model } = llm.value;
    if (!baseUrl.trim() || !apiKey.trim() || !model.trim()) {
      toast("请先填写 Base URL / API Key / 模型", "warn");
      return;
    }
    verifying.value = true;
    try {
      const report = await invoke<LlmVerifyReport>("verify_llm", { llm: { ...llm.value } });
      llm.value.thinkingOff = report.strategy;
      // 找不到关思考参数时明确警告（用户 2026-09-14）
      toast(report.message, report.strategy === "none" ? "warn" : "info");
    } catch (e) {
      toast(`验证失败：${String(e)}`, "error");
    } finally {
      verifying.value = false;
    }
  }

  /** 拉取 /models 列表（失败 toast 兜底；成功不打扰） */
  async function fetchModels(silent = false): Promise<void> {
    if (modelsFetching.value) return;
    const { baseUrl, apiKey } = llm.value;
    if (!baseUrl.trim() || !apiKey.trim()) {
      if (!silent) toast("请先填写 Base URL / API Key", "warn");
      return;
    }
    modelsFetching.value = true;
    try {
      modelOptions.value = await invoke<string[]>("fetch_llm_models", { baseUrl, apiKey });
      if (!silent) toast(`已获取 ${modelOptions.value.length} 个模型`);
    } catch (e) {
      toast(`获取模型列表失败：${String(e)}`, "error");
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
        updateText.value = `发现新版本 v${info.latest}`;
        toast(`发现新版本 v${info.latest}（当前 v${info.current}），可到 GitHub Releases 下载`, "info");
      } else {
        updateText.value = `已是最新（v${info.current}）`;
      }
    } catch (e) {
      updateText.value = "检查失败（仓库暂无发布或网络不可达）";
      if (manual) toast(`检查更新失败：${String(e)}`, "warn");
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
    openPage,
    save,
    setRepoPath,
    setTheme,
    verifyLlm,
    fetchModels,
    checkUpdate,
    ensureLoaded,
  };

});
