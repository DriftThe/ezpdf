import { defineStore } from "pinia";
import { ref } from "vue";
import { toast } from "../composables/toast";
import type { ServiceStatus } from "../types/domain";
import { invoke } from "@tauri-apps/api/core"
export const useParseStore = defineStore("parse", () => {
  /** 全局暂停/恢复（阶段4由 Rust 调度器驱动） */
  const paused = ref(false);
  /** ocr-server 连接状态（阶段3由 /health 健康检查驱动） */
  const serviceStatus = ref<ServiceStatus>("unknown");

  function togglePaused(): void {
    paused.value = !paused.value;
    toast(paused.value ? "解析已暂停" : "解析已恢复", paused.value ? "warn" : "info");
  }

  function setServiceStatus(s: ServiceStatus): void {
    serviceStatus.value = s;
  }

  const pythonReady = ref<boolean | null>(null);
  const cudaReady = ref<boolean | null>(null);
  /** 检查本地服务：探测 venv 解释器可用性，结果写入 pythonReady（阶段2.5 换 bootstrap 全量探测） */
  async function serve_check(): Promise<void> {
    pythonReady.value = false;
    cudaReady.value = false;
    try {
      pythonReady.value = await invoke<boolean>("check_python");
    } catch (e) {
      toast(String(e), "error");
      pythonReady.value = false;
    }
    try {
      cudaReady.value = await invoke<boolean>("check_cuda");
    } catch (e) {
      toast(String(e), "error");
      cudaReady.value = false;
    }
  }

  return { paused, serviceStatus, pythonReady, cudaReady, togglePaused, setServiceStatus, serve_check };
});
