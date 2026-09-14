<script setup lang="ts">
import { computed } from "vue";
import { useParseStore } from "../../../stores/parse";

const parse = useParseStore();

/** 状态位 → 文案 / 状态灯修饰类（"" = 灰色未就绪态） */
type EnvState = "notready" | "cpu" | "gpu";
type SvcState = "stopped" | "starting" | "idle" | "failed";

const pythonReady = computed(() => !!parse.envReport?.python);
const cudaReady = computed(() => !!parse.envReport?.gpu);
const envState = computed<EnvState>(() => {
  const report = parse.envReport;
  if (!report || report.missing.length > 0 || !report.torchBuild) return "notready";
  return report.torchBuild === "cuda" ? "gpu" : "cpu";
});
const modelsReady = computed(() => {
  const m = parse.envReport?.models;
  return !!m && m.layout && m.vl;
});
const svcState = computed<SvcState>(() => {
  switch (parse.serviceStatus) {
    case "starting":
      return "starting";
    case "connected":
      return "idle";
    case "failed":
      return "failed";
    default:
      return "stopped";
  }
});
const serviceBusy = computed(() => svcState.value === "starting" || svcState.value === "idle");

/** 灯 tooltip 明细（悬停可查具体版本/驱动/缺失项） */
const pythonTip = computed(() => parse.envReport?.pythonPath ?? "");
const cudaTip = computed(() => {
  const g = parse.envReport?.gpu;
  return g ? `${g.name} / 驱动 ${g.driver}${g.cuda ? ` / CUDA ${g.cuda}` : ""}` : "未检测到 nvidia-smi";
});
const envTip = computed(() => {
  const missing = parse.envReport?.missing ?? [];
  return missing.length > 0 ? `缺失: ${missing.join(", ")}` : (parse.envReport?.deps.torch ?? "");
});
const modelsTip = computed(() => {
  const m = parse.envReport?.models;
  if (!m) return "未检查";
  const missing = [!m.layout && "PP-DocLayoutV3", !m.vl && "PaddleOCR-VL-1.6"].filter(Boolean);
  return missing.length > 0 ? `缺模型目录: ${missing.join(", ")}` : "models/ 下两模型目录齐备";
});

const READY_LABEL: Record<"notready" | "ready", string> = { notready: "未就绪", ready: "已就绪" };
const READY_CLASS: Record<"notready" | "ready", string> = { notready: "", ready: "ok" };
const ENV_LABEL: Record<EnvState, string> = { notready: "未就绪", cpu: "CPU", gpu: "GPU" };
const ENV_CLASS: Record<EnvState, string> = { notready: "", cpu: "info", gpu: "ok" };
const SVC_LABEL: Record<SvcState, string> = {
  stopped: "未启动",
  starting: "启动中",
  idle: "空闲中",
  failed: "启动失败",
};
const SVC_CLASS: Record<SvcState, string> = { stopped: "", starting: "starting", idle: "ok", failed: "err" };

/** 五灯（Python/CUDA/环境/模型/服务）：就绪判定在脚本里做，模板只遍历 */
const lights = computed(() => [
  {
    key: "python",
    label: `Python ${READY_LABEL[pythonReady.value ? "ready" : "notready"]}`,
    cls: READY_CLASS[pythonReady.value ? "ready" : "notready"],
    tip: pythonTip.value,
  },
  {
    key: "cuda",
    label: `CUDA ${READY_LABEL[cudaReady.value ? "ready" : "notready"]}`,
    cls: READY_CLASS[cudaReady.value ? "ready" : "notready"],
    tip: cudaTip.value,
  },
  { key: "env", label: `环境 ${ENV_LABEL[envState.value]}`, cls: ENV_CLASS[envState.value], tip: envTip.value },
  {
    key: "models",
    label: `模型 ${READY_LABEL[modelsReady.value ? "ready" : "notready"]}`,
    cls: READY_CLASS[modelsReady.value ? "ready" : "notready"],
    tip: modelsTip.value,
  },
  {
    key: "service",
    label: `服务 ${SVC_LABEL[svcState.value]}`,
    cls: SVC_CLASS[svcState.value],
    tip: "pyserver 进程状态",
  },
]);
</script>

<template>
  <div class="set-pane">
    <h2 class="set-title">OCR服务</h2>
    <!-- 注意：含 button 的行不能用 label 包裹（label 会把整行点击转发给按钮） -->
    <div class="set-field">
      <span>环境检查</span>
      <button class="set-button" :disabled="parse.checking" @click="parse.checkEnv()">
        {{ parse.checking ? "检查中…" : "检查环境" }}
      </button>
    </div>
    <div class="set-field">
      <span>服务检查</span>
      <div class="set-field-row">
        <span v-for="light in lights" :key="light.key" class="svc" :class="light.cls" :title="light.tip">
          <span class="svc-dot" />{{ light.label }}
        </span>
      </div>
    </div>
    <div class="set-field">
      <span>环境操作</span>
      <div class="set-field-row">
        <button
          v-if="pythonReady && envState === 'notready'"
          class="set-button"
          :disabled="parse.installing"
          @click="parse.installEnv()"
        >
          {{ parse.installing ? "安装中…" : "一键安装" }}
        </button>
        <button
          v-if="pythonReady && !modelsReady"
          class="set-button"
          :disabled="parse.modelsBusy || parse.installing"
          @click="parse.downloadModels()"
        >
          {{ parse.modelsBusy ? "下载中…" : "下载模型（约1.9GB）" }}
        </button>
        <button v-if="!serviceBusy" class="set-button" @click="parse.startService()">启动服务</button>
        <button v-if="serviceBusy" class="set-button" @click="parse.stopService()">停止服务</button>
      </div>
    </div>
    <div class="set-field">
      <span>日志</span>
      <pre class="log-box">{{ parse.envLogs.join("\n") || "暂无日志" }}</pre>
    </div>
  </div>
</template>
