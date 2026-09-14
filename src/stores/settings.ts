import { defineStore } from "pinia";
import { ref } from "vue";
import { toast } from "../composables/toast";
import { useLibraryStore } from "./library";

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
  void fillLlmFromAuthCfg(llm);

  const ocr = ref<OcrSettings>({
    autoLaunch: true,
  });

  const parse = ref<ParseSettings>({
    ocrConcurrency: 1,
    llmConcurrency: 2,
    resumeOnStart: true,
  });

  function openPage(): void {
    pageOpen.value = true;
    // sidebar 收起时工具栏满宽，会盖住设置页左列顶部的返回键——打开设置先复位展开
    useLibraryStore().sidebarOpen = true;
  }
  function closePage(): void {
    pageOpen.value = false;
  }
  /** 保存并退出：现绑在设置页返回按钮上；阶段1起改为写入 appDataDir/settings.json + keyring 存 key */
  function save(): void {
    toast("设置已保存（持久化将在阶段1接入）");
    pageOpen.value = false;
  }

  return { pageOpen, section, llm, ocr, parse, openPage, closePage, save };
});
