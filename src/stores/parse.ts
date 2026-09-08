import { defineStore } from "pinia";
import { ref } from "vue";
import { toast } from "../composables/toast";
import type { ServiceStatus } from "../types/domain";

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

  /** 阶段3：invoke Rust → /health */
  function testOcrConnection(): void {
    toast("阶段3接入：OCR 服务健康检查");
  }

  return { paused, serviceStatus, togglePaused, setServiceStatus, testOcrConnection };
});
