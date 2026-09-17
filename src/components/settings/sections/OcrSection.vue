<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useParseStore } from "../../../stores/parse";
import { useSettingsStore } from "../../../stores/settings";
import SelectArrow from "../../common/SelectArrow.vue";

const parse = useParseStore();
const settings = useSettingsStore();
const { t } = useI18n();

/** Install mode: follows the hardware by default (a detected GPU preselects GPU) */
const installMode = ref<"cpu" | "gpu">("cpu");
watch(
  () => parse.envReport?.gpu,
  (gpu) => {
    if (gpu) installMode.value = "gpu";
  },
  { immediate: true },
);

// Refresh the report when the OCR settings page opens (lights and button availability follow the latest probe)
// Online mode doesn't probe the local environment (that's the service's job), only the URL
onMounted(() => {
  if (!online.value && parse.envReport === null) void parse.checkEnv();
});

/** Online mode: hides the local environment/install area, shows URL + test instead */
const online = computed(() => settings.ocr.mode === "online");
const probing = ref(false);
async function onTest(): Promise<void> {
  probing.value = true;
  try {
    await parse.testRemote();
  } finally {
    probing.value = false;
  }
}

/** State → label / status-light modifier class ("" = grey not-ready state) */
type EnvState = "notready" | "cpu" | "gpu";
type SvcState = "stopped" | "starting" | "busy" | "idle" | "failed";

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
      // Service online: an in-flight batch = busy (reported: the local service was running a batch but kept showing idle)
      return parse.parsing ? "busy" : "idle";
    case "failed":
      return "failed";
    default:
      return "stopped";
  }
});
/** Shapes in which the service is "held": starting/online (including busy) → button shows "stop service" */
const serviceBusy = computed(
  () => svcState.value === "starting" || svcState.value === "busy" || svcState.value === "idle",
);

/** Light tooltip details (hover for version/driver/missing items) */
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
  busy: t("ocr.svcBusy"),
  idle: t("ocr.svcIdle"),
  failed: t("ocr.svcFailed"),
}));
const SVC_CLASS: Record<SvcState, string> = {
  stopped: "",
  starting: "starting",
  busy: "busy",
  idle: "ok",
  failed: "err",
};
const svcTip = computed(() => (svcState.value === "busy" ? t("ocr.svcBusyTip") : t("ocr.pyserverTip")));

/** Five lights (Python/CUDA/env/models/service): readiness is computed here, the template only iterates */
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
    tip: svcTip.value,
  },
]);
</script>

<template>
  <div class="set-pane">
    <h2 class="set-title">{{ t("ocr.title") }}</h2>

    <!-- Service source: local = bundled pyserver; online = a remote HTTP service of the same kind.
         Online mode only needs a reachable URL, so a bare base environment (no deps/models) works;
         translation is unrelated to it -->
    <div class="set-field">
      <span>{{ t("ocr.mode") }}</span>
      <span class="set-select-wrap">
        <select v-model="settings.ocr.mode" class="set-select">
          <option value="local">{{ t("ocr.modeLocal") }}</option>
          <option value="online">{{ t("ocr.modeOnline") }}</option>
        </select>
        <SelectArrow />
      </span>
    </div>

    <!-- Online mode: URL field + "test" on the right (probes only, doesn't change the connection state) + start service to register -->
    <template v-if="online">
      <div class="set-field">
        <span>{{ t("ocr.url") }}</span>
        <div class="set-field-row url-row">
          <input v-model="settings.ocr.url" class="url-input" :placeholder="t('ocr.urlPlaceholder')" />
          <button class="set-button" :disabled="probing" @click="onTest">
            {{ probing ? t("ocr.checking") : t("ocr.testUrl") }}
          </button>
        </div>
      </div>
      <!-- Service token (only for the deployed server form, see pyserver/app/server_docker.py): the server
           prints the token to the terminal on every start, paste it here; leave empty if the server has no auth -->
      <div class="set-field">
        <span>{{ t("ocr.token") }}</span>
        <div class="set-field-row url-row">
          <input v-model="settings.ocr.token" class="url-input" :placeholder="t('ocr.tokenPlaceholder')" />
        </div>
      </div>
      <!-- Batch page count advertised by the server (appears after test/connect; re-handshaked before every OCR request) -->
      <p v-if="parse.onlineHealth" class="set-hint batch-hint">
        {{ t("ocr.batchHint", { batch: parse.onlineHealth.maxBatchPages }) }}
      </p>
    </template>

    <template v-else>
      <!-- Note: rows containing a button must not be wrapped in a label (a label forwards clicks on the whole row to the button) -->
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
      <!-- One-click install service: install-mode dropdown (custom-styled) + mirror switch + progress -->
      <div class="set-field">
        <span>{{ t("ocr.installService") }}</span>
        <div class="set-field-row install-row">
          <span class="set-select-wrap">
            <select v-model="installMode" class="set-select" :title="t('ocr.installMode')">
              <option value="gpu">GPU</option>
              <option value="cpu">CPU</option>
            </select>
            <SelectArrow />
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
    </template>

    <div class="set-field">
      <span>{{ t("ocr.serviceActions") }}</span>
      <div class="set-field-row">
        <button v-if="!serviceBusy" class="set-button" @click="parse.startService()">{{ t("ocr.startService") }}</button>
        <button v-if="serviceBusy" class="set-button" @click="parse.stopService()">{{ t("ocr.stopService") }}</button>
      </div>
    </div>
    <!-- Logs only in local-hosted mode: an online service keeps its own logs server-side -->
    <div v-if="!online" class="set-field">
      <span>{{ t("settings.log") }}</span>
      <pre class="log-box">{{ parse.envLogs.join("\n") || t("settings.noLogs") }}</pre>
    </div>
  </div>
</template>

<style scoped>
/* Batch-size hint: same visual layer as the URL field, de-emphasized (server-advertised value) */
.batch-hint {
  color: var(--accent);
}
/* URL field fills the remaining width, "test" button stays on the right */
.url-row {
  gap: 12px;
  align-items: center;
}
.url-input {
  flex: 1;
  min-width: 0;
}
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
  margin: -2px 0 6px 120px; /* aligns with .set-field's 110px label column + 10px gap */
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
