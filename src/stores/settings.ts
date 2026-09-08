import { defineStore } from "pinia";
import { ref } from "vue";
import { toast } from "../composables/toast";

export interface LlmSettings {
  baseUrl: string;
  apiKey: string; // 阶段1起改存系统凭据库（keyring）
  model: string;
  targetLang: string;
}

export interface OcrSettings {
  serverUrl: string;
  launchCommand: string;
  autoLaunch: boolean;
}

export interface ParseSettings {
  ocrConcurrency: number;
  llmConcurrency: number;
  resumeOnStart: boolean;
}

export const useSettingsStore = defineStore("settings", () => {
  const modalOpen = ref(false);

  const llm = ref<LlmSettings>({
    baseUrl: "https://api.deepseek.com/v1",
    apiKey: "",
    model: "deepseek-chat",
    targetLang: "zh",
  });

  const ocr = ref<OcrSettings>({
    serverUrl: "http://127.0.0.1:8790",
    launchCommand: "uv run --project ocr-server uvicorn app:app --port 8790",
    autoLaunch: true,
  });

  const parse = ref<ParseSettings>({
    ocrConcurrency: 1,
    llmConcurrency: 2,
    resumeOnStart: true,
  });

  function openModal(): void {
    modalOpen.value = true;
  }
  function closeModal(): void {
    modalOpen.value = false;
  }
  /** 阶段1：写入 appDataDir/settings.json + keyring 存 key */
  function save(): void {
    toast("设置已保存（持久化将在阶段1接入）");
    modalOpen.value = false;
  }

  return { modalOpen, llm, ocr, parse, openModal, closeModal, save };
});
