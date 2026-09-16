<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useParseStore } from "../../../stores/parse";
import { useSettingsStore } from "../../../stores/settings";
import {
  CUSTOM_PROVIDER,
  SUPPORTED_API,
  providerLabel,
  thinkingHint,
} from "../../../lib/piModels";
import { CUSTOM_TARGET_LANG, TARGET_LANG_OPTIONS, isPresetTargetLang } from "../../../lib/languages";
import SelectArrow from "../../common/SelectArrow.vue";

const settings = useSettingsStore();
const parse = useParseStore();
const { t } = useI18n();

// 目录懒加载（~400KB 单独 chunk）：设置页打开 LLM 面板才拉
onMounted(() => void settings.ensureCatalog());

const isCustom = computed(() => settings.llm.provider === CUSTOM_PROVIDER);

/** 兼容性/关思考说明：预设已定则不需要靠验证按钮慢慢试 */
const hint = computed(() => thinkingHint(settings.llm.preset, settings.llm.thinkingOff));
const unsupported = computed(
  () => !!settings.llm.preset.api && settings.llm.preset.api !== SUPPORTED_API,
);
/** 当前供应商是「协议不支持」那一类（旧配置残留）：下拉里只留一个禁用项显示它 */
const unsupportedProvider = computed(() =>
  settings.otherProviders.some((p) => p.id === settings.llm.provider),
);

function onProvider(e: Event): void {
  settings.applyProvider((e.target as HTMLSelectElement).value);
}

/** 目标语言：预设命中就直接用，否则显示"自定义"并展开输入框（用户 2026-09-15） */
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

    <!-- 启用翻译（用户 2026-09-15）：首项总开关；关闭时 Rust 把 OCR 文本原文记入译文列并
         标记完成（不请求模型、也不留 null），重新打开翻译也不会回翻这些页面 -->
    <label class="set-check">
      <input v-model="settings.llm.translateEnabled" type="checkbox" />
      <span>{{ t("llm.translateEnabled") }}</span>
    </label>
    <p class="set-hint">{{ t("llm.translateEnabledHint") }}</p>

    <!--
      供应商预设（用户 2026-09-15 整合 pi-ai 目录）：选供应商 → 自动填端点 + 列出预设模型，
      并把「协议 / max tokens 字段 / 关思考参数形态」派生给 Rust 客户端（lib/piModels.ts）。
      协议不是 openai-completions 的供应商不列出（客户端只实现了这一种协议，见 docs/protocols.md）；
      旧配置若正指向这类供应商，只把它作为禁用项显示，避免下拉框空掉。
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

    <!-- 预设元信息：端点 / Key 环境变量 / 关思考形态 -->
    <p class="set-hint llm-meta" :class="{ err: unsupported }">
      <template v-if="!isCustom && settings.presetProvider">
        <code class="llm-url">{{ settings.llm.baseUrl }}</code>
        <span v-if="settings.presetProvider.envKeys.length">{{ t("llm.envKeys", { keys: settings.presetProvider.envKeys.join(" / ") }) }}</span>
      </template>
      {{ hint }}
    </p>

    <label v-if="isCustom" class="set-field">
      <span>{{ t("llm.baseUrl") }}</span>
      <input v-model="settings.llm.baseUrl" placeholder="https://api.deepseek.com/v1" />
    </label>

    <!-- API Key + 验证（用户 2026-09-14）：验证按钮检查连通性并探测关闭思考的参数策略；
         预设已给出关思考形态时无需靠探测（策略仍会回写，作为显式覆盖） -->
    <div class="set-field-row llm-row">
      <label class="set-field">
        <span>{{ t("llm.apiKey") }}</span>
        <input v-model="settings.llm.apiKey" type="password" :placeholder="t('llm.apiKeyPlaceholder')" />
      </label>
      <button class="set-button llm-verify" :disabled="settings.verifying" @click="settings.verifyLlm()">
        {{ settings.verifying ? t("llm.verifying") : t("llm.verify") }}
      </button>
    </div>

    <!-- 模型：手填输入（「获取在线列表」补全 datalist；命中 pi-ai 目录就走预设快照） -->
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
/* 验证/获取按钮与输入行对齐（底部对齐，与输入框同高；行内 gap 收窄到 8px） */
.llm-row {
  gap: 8px;
  align-items: flex-end;
  margin: 9px 0;
}
.llm-row .set-field {
  margin: 0; /* 行内 field 的纵向 margin 交给 .llm-row，避免行高翻倍 */
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
.llm-meta.err {
  color: var(--err);
}
.llm-url {
  font-size: 11px;
  color: var(--text-2);
  background: var(--bg-hover);
  padding: 1px 6px;
  border-radius: var(--radius-sm);
}
</style>
