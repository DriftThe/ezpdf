import { defineStore } from "pinia";
import { ref } from "vue";
import { toast } from "../composables/toast";
import { useLibraryStore } from "./library";

export interface LlmSettings {
  baseUrl: string;
  apiKey: string; // 阶段1起改存系统凭据库（keyring）
  model: string;
  targetLang: string;
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

export const useSettingsStore = defineStore("settings", () => {
  /** 设置整页是否打开（覆盖 sidebar + reader 视窗，保留顶部工具栏；不卸载原视窗） */
  const pageOpen = ref(false);
  /** 当前选中的设置子项 */
  const section = ref<SettingsSection>("llm");

  const llm = ref<LlmSettings>({
    baseUrl: "https://api.deepseek.com/v1",
    apiKey: "",
    model: "deepseek-chat",
    targetLang: "zh",
  });

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
