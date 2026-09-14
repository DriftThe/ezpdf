<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useParseStore } from "../../../stores/parse";
import { useSettingsStore } from "../../../stores/settings";

const parse = useParseStore();
const settings = useSettingsStore();

/** 安装模式（用户 2026-09-14）：默认跟硬件走（探测到 GPU 自动选 GPU） */
const installMode = ref<"cpu" | "gpu">("cpu");
watch(
  () => parse.envReport?.gpu,
  (gpu) => {
    if (gpu) installMode.value = "gpu";
  },
  { immediate: true },
);

// 打开 OCR 设置页即刷新一次报告（灯与按钮可用性以最新探测为准）
onMounted(() => {
  if (parse.envReport === null) void parse.checkEnv();
});

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
  return missing.length > 0 ? `缺模型目录: ${missing.join(", ")}` : "模型目录齐备";
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
    <!-- 一键安装服务（用户 2026-09-14）：下拉选择安装模式（自绘样式）+ 镜像源开关 + 进度 -->
    <div class="set-field">
      <span>安装服务</span>
      <div class="set-field-row install-row">
        <span class="set-select-wrap">
          <select v-model="installMode" class="set-select" title="安装模式">
            <option value="gpu">GPU（CUDA cu132）</option>
            <option value="cpu">CPU</option>
          </select>
          <svg class="set-select-arrow" viewBox="0 0 10 6" width="10" height="6" aria-hidden="true">
            <path d="M1 1l4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
          </svg>
        </span>
        <label class="set-check">
          <input v-model="settings.ocr.installMirror" type="checkbox" />
          <span>使用镜像源</span>
        </label>
        <button
          class="set-button"
          :disabled="parse.installing"
          @click="parse.installService(installMode, settings.ocr.installMirror)"
        >
          {{ parse.installing ? "安装中…" : "一键安装服务" }}
        </button>
        <span v-if="parse.installing && parse.installProgress" class="install-phase">
          {{ parse.installProgress.phase }} {{ parse.installProgress.percent }}%
        </span>
      </div>
    </div>
    <div v-if="parse.installing && parse.installProgress" class="progress-track">
      <div class="progress-fill" :style="{ width: parse.installProgress.percent + '%' }" />
    </div>
    <p class="set-hint">
      安装内容 = 基础依赖 + torch（{{ installMode === "gpu" ? "GPU/CUDA 版，约 3GB" : "CPU 版，约 200MB" }}）+ 两个模型（约 1.9GB）。
      GPU 模式要求 nvidia-smi 可用（无则中止并提示）；网络受限时保持「使用镜像源」开启。
    </p>
    <div class="set-field">
      <span>服务操作</span>
      <div class="set-field-row">
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

<style scoped>
.install-row {
  gap: 12px;
  align-items: center;
}
.install-phase {
  font-size: 12px;
  color: var(--text-2);
  white-space: nowrap;
}
.progress-track {
  height: 4px;
  margin: -2px 0 6px 120px; /* 与 .set-field 的 110px 标签列 + 10px gap 对齐 */
  border-radius: 2px;
  background: var(--bg-hover);
  overflow: hidden;
}
.progress-fill {
  height: 100%;
  background: var(--accent);
  transition: width 0.3s ease;
}
</style>
