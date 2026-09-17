<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useParseStore } from "../../../stores/parse";
import { useSettingsStore } from "../../../stores/settings";
import {
  CUSTOM_PROVIDER,
  PROTOCOL_LABELS,
  SUPPORTED_APIS,
  providerLabel,
} from "../../../lib/piModels";
import { CUSTOM_TARGET_LANG, TARGET_LANG_OPTIONS, isPresetTargetLang } from "../../../lib/languages";
import SelectArrow from "../../common/SelectArrow.vue";

const settings = useSettingsStore();
const parse = useParseStore();
const { t } = useI18n();

// Catalog lazy-load (~400KB separate chunk): fetched only when the LLM panel opens
onMounted(() => void settings.ensureCatalog());

const isCustom = computed(() => settings.llm.provider === CUSTOM_PROVIDER);

/** Current provider is one of the "unsupported protocol" kind (stale config): show it as a single disabled option */
const unsupportedProvider = computed(() =>
  settings.otherProviders.some((p) => p.id === settings.llm.provider),
);

function onProvider(e: Event): void {
  settings.applyProvider((e.target as HTMLSelectElement).value);
}

function onProtocol(e: Event): void {
  settings.applyProtocol((e.target as HTMLSelectElement).value);
}

/** Base URL example follows the protocol (custom endpoints most often get the path wrong) */
const baseUrlPlaceholder = computed(() => {
  switch (settings.protocol) {
    case "anthropic-messages":
      return "https://api.anthropic.com";
    case "openai-responses":
      return "https://api.openai.com/v1";
    default:
      return "https://api.deepseek.com/v1";
  }
});

/** Target language: use a preset hit directly, otherwise show "custom" and expand the input */
const customLang = ref(false);
const langPick = computed(() => {
  if (customLang.value) return CUSTOM_TARGET_LANG;
  return isPresetTargetLang(settings.llm.targetLang) ? settings.llm.targetLang : CUSTOM_TARGET_LANG;
});

function onPickLang(e: Event): void {
  const v = (e.target as HTMLSelectElement).value;
  customLang.value = v === CUSTOM_TARGET_LANG;
  if (!customLang.value) settings.llm.targetLang = v;
}
</script>

<template>
  <div class="set-pane">
    <h2 class="set-title">{{ t("llm.title") }}</h2>

    <!-- Enable translation: the master switch; when off, Rust records the OCR text into the
         translation column and marks it done (no model request, no null), so re-enabling never
         re-translates those pages -->
    <label class="set-check">
      <input v-model="settings.llm.translateEnabled" type="checkbox" />
      <span>{{ t("llm.translateEnabled") }}</span>
    </label>

    <!--
      Provider presets (integration of the pi-ai catalog): picking a provider fills the endpoint and
      lists its preset models, and derives "protocol / max tokens field / thinking-off param shape"
      for the Rust client (lib/piModels.ts). Providers with an unsupported protocol (google/bedrock/
      mistral/azure etc.) are not listed; a stale config pointing at one shows it as a disabled option
      so the dropdown isn't empty.
    -->
    <div class="set-field">
      <span>{{ t("llm.provider") }}</span>
      <span class="set-select-wrap">
        <select class="set-select llm-provider" :value="settings.llm.provider" @change="onProvider">
          <option v-for="p in settings.usableProviders" :key="p.id" :value="p.id">
            {{ providerLabel(p.id) }}
          </option>
          <option v-if="unsupportedProvider" :value="settings.llm.provider" disabled>
            {{ t("llm.providerUnavailable", { name: providerLabel(settings.llm.provider) }) }}
          </option>
          <option :value="CUSTOM_PROVIDER">{{ t("llm.providerCustom") }}</option>
        </select>
        <SelectArrow />
      </span>
    </div>

    <!--
      Protocol: the three wire protocols each have their own request body/auth headers/field
      extraction (see docs/protocols.md). Preset models get their protocol from the pi-ai
      catalog and show it read-only; only "custom" lets you pick one and auto-fills the Base URL path.
    -->
    <div class="set-field">
      <span>{{ t("llm.protocol") }}</span>
      <span class="set-select-wrap">
        <select class="set-select" :value="settings.protocol" :disabled="!isCustom" @change="onProtocol">
          <option v-for="api in SUPPORTED_APIS" :key="api" :value="api">
            {{ PROTOCOL_LABELS[api] }}
          </option>
        </select>
        <SelectArrow />
      </span>
    </div>
    <p v-if="isCustom" class="set-hint">{{ t("llm.protocolHintCustom") }}</p>

    <!-- Preset endpoint (thinking-off/env-var explanations are no longer shown) -->
    <p v-if="!isCustom && settings.presetProvider" class="set-hint llm-meta">
      <code class="llm-url">{{ settings.llm.baseUrl }}</code>
    </p>

    <label v-if="isCustom" class="set-field">
      <span>{{ t("llm.baseUrl") }}</span>
      <input v-model="settings.llm.baseUrl" :placeholder="baseUrlPlaceholder" />
    </label>

    <!-- API Key + verify: the verify button checks connectivity and probes the thinking-off
         parameter strategy; a preset that already declares its shape needs no probe
         (the strategy is still written back as an explicit override) -->
    <div class="set-field-row llm-row">
      <label class="set-field">
        <span>{{ t("llm.apiKey") }}</span>
        <input v-model="settings.llm.apiKey" type="password" :placeholder="t('llm.apiKeyPlaceholder')" />
      </label>
      <button class="set-button llm-verify" :disabled="settings.verifying" @click="settings.verifyLlm()">
        {{ settings.verifying ? t("llm.verifying") : t("llm.verify") }}
      </button>
    </div>

    <!-- Model: free-text input ("fetch online list" fills the datalist; a catalog hit uses the preset snapshot) -->
    <div class="set-field-row llm-row">
      <label class="set-field">
        <span>{{ t("llm.model") }}</span>
        <input
          v-model="settings.llm.model"
          list="llm-models"
          placeholder="deepseek-chat"
          @change="settings.applyModelInput()"
        />
        <datalist id="llm-models">
          <option v-for="m in settings.modelOptions" :key="m" :value="m" />
        </datalist>
      </label>
      <button class="set-button llm-verify" :disabled="settings.modelsFetching" @click="settings.fetchModels()">
        {{ settings.modelsFetching ? t("llm.fetching") : t("llm.fetchModels") }}
      </button>
    </div>

    <div class="set-field">
      <span>{{ t("llm.targetLang") }}</span>
      <span class="set-select-wrap">
        <select class="set-select" :value="langPick" @change="onPickLang">
          <option v-for="o in TARGET_LANG_OPTIONS" :key="o.value" :value="o.value">{{ o.label }}</option>
          <option :value="CUSTOM_TARGET_LANG">{{ t("llm.targetLangCustom") }}</option>
        </select>
        <SelectArrow />
      </span>
    </div>
    <label v-if="langPick === CUSTOM_TARGET_LANG" class="set-field">
      <span>{{ t("llm.targetLangCustomLabel") }}</span>
      <input v-model="settings.llm.targetLang" :placeholder="t('llm.targetLangPlaceholder')" />
    </label>
    <label class="set-check">
      <input v-model="settings.llm.smartContext" type="checkbox" />
      <span>{{ t("llm.smartContext") }}</span>
    </label>
    <div class="set-field">
      <span>{{ t("settings.log") }}</span>
      <pre class="log-box">{{ parse.llmLogs.join("\n") || t("settings.noLogs") }}</pre>
    </div>
  </div>
</template>

<style scoped>
/* Verify/fetch buttons align with the input row (bottom-aligned, same height as inputs; inline gap narrows to 8px) */
.llm-row {
  gap: 8px;
  align-items: flex-end;
  margin: 9px 0;
}
.llm-row .set-field {
  margin: 0; /* the inline field's vertical margin goes to .llm-row, avoiding double row height */
}
.llm-verify {
  flex: none;
  margin-bottom: 1px;
}
.llm-provider {
  width: 100%;
}
.llm-meta {
  display: flex;
  flex-wrap: wrap;
  gap: 4px 12px;
  align-items: center;
  margin: 2px 0 10px;
}
.llm-url {
  font-size: 11px;
  color: var(--text-2);
  background: var(--bg-hover);
  padding: 1px 6px;
  border-radius: var(--radius-sm);
}
</style>
