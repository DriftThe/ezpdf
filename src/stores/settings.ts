import { defineStore } from "pinia";
import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
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
}

export interface OcrSettings {
  /** 随应用启动拉起 pyserver（端口/命令由 Rust 固定拼装，不暴露给用户） */
  autoLaunch: boolean;
}

export interface ParseSettings {
  ocrConcurrency: number;
  llmConcurrency: number;
  resumeOnStart: boolean;
}

/**
 * 设置页左侧导航子项。
 * 新增设置块三步：① 此处扩展 union（如 "theme"）
 * ② 新建 src/components/settings/sections/ThemeSection.vue（抄现有 section 当模板，
 *    根元素 class="set-pane"；表单状态在本 store 加 ref）
 * ③ 在 SettingsPage.vue 的 SECTIONS 注册表加一行 —— union 扩了不注册会编译报错
 */
export type SettingsSection = "llm" | "ocr" | "parse" | "common";

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
  if (!import.meta.env.DEV) return;
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
  ocr?: Partial<OcrSettings>;
  parse?: Partial<ParseSettings>;
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
  });

  const ocr = ref<OcrSettings>({
    autoLaunch: true,
  });

  const parse = ref<ParseSettings>({
    ocrConcurrency: 1,
    llmConcurrency: 2,
    resumeOnStart: true,
  });

  /**
   * 读取持久化设置（config.json；dev = 仓库根、生产 = ~/.ezpdf）：
   * auth.cfg（dev）先落地，持久化配置覆盖其上——应用内改过的值优先于开发默认。
   * 单次幂等（共享 promise），App 启动与设置页打开都安全。
   */
  let loadPromise: Promise<void> | null = null;
  function ensureLoaded(): Promise<void> {
    loadPromise ??= (async () => {
      if (!isTauri) return;
      try {
        const text = await invoke<string | null>("load_settings");
        if (!text) return;
        const cfg = JSON.parse(text) as PersistedConfig;
        mergeSection(llm.value, cfg.llm);
        mergeSection(ocr.value, cfg.ocr);
        mergeSection(parse.value, cfg.parse);
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

  /** 保存并退出（设置页返回键）：整份写入 config.json，失败则留在设置页报错 */
  async function save(): Promise<void> {
    if (!isTauri) {
      pageOpen.value = false;
      return;
    }
    try {
      const payload: PersistedConfig = {
        llm: { ...llm.value },
        ocr: { ...ocr.value },
        parse: { ...parse.value },
      };
      await invoke("save_settings", { json: JSON.stringify(payload, null, 2) });
      toast("设置已保存");
      pageOpen.value = false;
    } catch (e) {
      toast(`设置保存失败：${String(e)}`, "error");
    }
  }

  return { pageOpen, section, llm, ocr, parse, openPage, save, ensureLoaded };

});
