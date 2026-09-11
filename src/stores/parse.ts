import { defineStore } from "pinia";
import { ref } from "vue";
import { toast } from "../composables/toast";
import type { OcrEnvReport, ServiceStatus } from "../types/domain";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export const useParseStore = defineStore("parse", () => {
  /** 全局暂停/恢复（阶段4由 Rust 调度器驱动） */
  const paused = ref(false);
  /** pyserver 生命周期状态（ocr://status 事件驱动；未知/断线/失败均显示灰/红） */
  const serviceStatus = ref<ServiceStatus>("unknown");
  /** 环境分层报告（ocr_env_report 的 bootstrap JSON；null = 未检查） */
  const envReport = ref<OcrEnvReport | null>(null);
  /** 服务/安装日志流（ocr://log；环形截断保尾 200 行） */
  const envLogs = ref<string[]>([]);

  const checking = ref(false);
  const installing = ref(false);

  function togglePaused(): void {
    paused.value = !paused.value;
    toast(paused.value ? "解析已暂停" : "解析已恢复", paused.value ? "warn" : "info");
  }

  function pushLog(line: string): void {
    envLogs.value.push(line);
    if (envLogs.value.length > 200) {
      envLogs.value.splice(0, envLogs.value.length - 200);
    }
  }

  // 事件订阅（store 单例创建一次即完成；非 Tauri 环境（纯浏览器 dev）静默失败）
  listen<string>("ocr://log", (e) => pushLog(e.payload)).catch(() => undefined);
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

  /** 一键安装：venv 创建（必要时）→ 基础依赖 → torch 变体；过程经 ocr://log 流式展示 */
  async function installEnv(): Promise<void> {
    if (installing.value) return;
    installing.value = true;
    try {
      await invoke("ocr_install_env");
      await checkEnv();
      toast("环境安装完成", "info");
    } catch (e) {
      toast(String(e), "error");
    } finally {
      installing.value = false;
    }
  }

  async function startService(): Promise<void> {
    try {
      await invoke("ocr_start");
    } catch (e) {
      toast(String(e), "error");
    }
  }

  async function stopService(): Promise<void> {
    try {
      await invoke("ocr_stop");
    } catch (e) {
      toast(String(e), "error");
    }
  }

  return {
    paused,
    serviceStatus,
    togglePaused,
    envReport,
    envLogs,
    checking,
    installing,
    checkEnv,
    installEnv,
    startService,
    stopService,
  };
});
