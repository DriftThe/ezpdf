<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useParseStore } from "../../../stores/parse";
import { useSettingsStore } from "../../../stores/settings";

const parse = useParseStore();
const settings = useSettingsStore();
const { t } = useI18n();

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
  return g
    ? `${g.name} / ${t("ocr.driver")} ${g.driver}${g.cuda ? ` / CUDA ${g.cuda}` : ""}`
    : t("ocr.noNvidiaSmi");
});
const envTip = computed(() => {
  const missing = parse.envReport?.missing ?? [];
  return missing.length > 0
    ? t("ocr.missing", { items: missing.join(", ") })
    : (parse.envReport?.deps.torch ?? "");
});
const modelsTip = computed(() => {
  const m = parse.envReport?.models;
  if (!m) return t("ocr.notChecked");
  const missing = [!m.layout && "PP-DocLayoutV3", !m.vl && "PaddleOCR-VL-1.6"].filter(Boolean);
  return missing.length > 0
    ? t("ocr.missingModels", { items: missing.join(", ") })
    : t("ocr.modelsReady");
});

const READY_CLASS: Record<"notready" | "ready", string> = { notready: "", ready: "ok" };
const ENV_LABEL = computed<Record<EnvState, string>>(() => ({
  notready: t("ocr.notReady"),
  cpu: "CPU",
  gpu: "GPU",
}));
const ENV_CLASS: Record<EnvState, string> = { notready: "", cpu: "info", gpu: "ok" };
const SVC_LABEL = computed<Record<SvcState, string>>(() => ({
  stopped: t("ocr.svcStopped"),
  starting: t("ocr.svcStarting"),
  idle: t("ocr.svcIdle"),
  failed: t("ocr.svcFailed"),
}));
const SVC_CLASS: Record<SvcState, string> = { stopped: "", starting: "starting", idle: "ok", failed: "err" };

/** 五灯（Python/CUDA/环境/模型/服务）：就绪判定在脚本里做，模板只遍历 */
const lights = computed(() => [
  {
    key: "python",
    label: `Python ${pythonReady.value ? t("ocr.ready") : t("ocr.notReady")}`,
    cls: READY_CLASS[pythonReady.value ? "ready" : "notready"],
    tip: pythonTip.value,
  },
  {
    key: "cuda",
    label: `CUDA ${cudaReady.value ? t("ocr.ready") : t("ocr.notReady")}`,
    cls: READY_CLASS[cudaReady.value ? "ready" : "notready"],
    tip: cudaTip.value,
  },
  { key: "env", label: `${t("ocr.lightEnv")} ${ENV_LABEL.value[envState.value]}`, cls: ENV_CLASS[envState.value], tip: envTip.value },
  {
    key: "models",
    label: `${t("ocr.lightModels")} ${modelsReady.value ? t("ocr.ready") : t("ocr.notReady")}`,
    cls: READY_CLASS[modelsReady.value ? "ready" : "notready"],
    tip: modelsTip.value,
  },
  {
    key: "service",
    label: `${t("ocr.lightService")} ${SVC_LABEL.value[svcState.value]}`,
    cls: SVC_CLASS[svcState.value],
    tip: t("ocr.pyserverTip"),
  },
]);
</script>

<template>
  <div class="set-pane">
    <h2 class="set-title">{{ t("ocr.title") }}</h2>
    <!-- 注意：含 button 的行不能用 label 包裹（label 会把整行点击转发给按钮） -->
    <div class="set-field">
      <span>{{ t("ocr.envCheck") }}</span>
      <button class="set-button" :disabled="parse.checking" @click="parse.checkEnv()">
        {{ parse.checking ? t("ocr.checking") : t("ocr.checkEnv") }}
      </button>
    </div>
    <div class="set-field">
      <span>{{ t("ocr.serviceCheck") }}</span>
      <div class="set-field-row">
        <span v-for="light in lights" :key="light.key" class="svc" :class="light.cls" :title="light.tip">
          <span class="svc-dot" />{{ light.label }}
        </span>
      </div>
    </div>
    <!-- 一键安装服务（用户 2026-0x9-14）：下拉选择安装模式（自绘样式）+ 镜像源开关 + 进度 -->
    <div class="set-field">
      <span>{{ t("ocr.installService") }}</span>
      <div class="set-field-row install-row">
        <span class="set-select-wrap">
          <select v-model="installMode" class="set-select" :title="t('ocr.installMode')">
            <option value="gpu">GPU</option>
            <option value="cpu">CPU</option>
          </select>
          <svg class="set-select-arrow" viewBox="0 0 10 6" width="10" height="6" aria-hidden="true">
            <path d="M1 1l4 4 4-4" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
          </svg>
        </span>
        <label class="set-check">
          <input v-model="settings.ocr.installMirror" type="checkbox" />
          <span>{{ t("ocr.useMirror") }}</span>
        </label>
        <button
          class="set-button"
          :disabled="parse.installing"
          @click="parse.installService(installMode, settings.ocr.installMirror)"
        >
          {{ parse.installing ? t("ocr.installing") : t("ocr.installOneClick") }}
        </button>
        <span v-if="parse.installing && parse.installProgress" class="install-phase">
          {{ parse.installProgress.phase }} {{ parse.installProgress.percent }}%
        </span>
      </div>
    </div>
    <div v-if="parse.installing && parse.installProgress" class="progress-track">
      <div class="progress-fill" :style="{ width: parse.installProgress.percent + '%' }" />
    </div>
    <div class="set-field">
      <span>{{ t("ocr.serviceActions") }}</span>
      <div class="set-field-row">
        <button v-if="!serviceBusy" class="set-button" @click="parse.startService()">{{ t("ocr.startService") }}</button>
        <button v-if="serviceBusy" class="set-button" @click="parse.stopService()">{{ t("ocr.stopService") }}</button>
      </div>
    </div>
    <div class="set-field">
      <span>{{ t("settings.log") }}</span>
      <pre class="log-box">{{ parse.envLogs.join("\n") || t("settings.noLogs") }}</pre>
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
