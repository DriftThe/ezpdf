<script setup lang="ts">
import { onMounted } from "vue";
import { useParseStore } from "../../../stores/parse";
import { useSettingsStore } from "../../../stores/settings";

const settings = useSettingsStore();
const parse = useParseStore();

/** 自动拉取模型列表：每会话只静默尝试一次（失败会 toast，不反复打扰） */
let autoFetchDone = false;
onMounted(() => {
  if (autoFetchDone) return;
  autoFetchDone = true;
  void settings.fetchModels(true);
});
</script>

<template>
  <div class="set-pane">
    <h2 class="set-title">LLM 翻译</h2>
    <label class="set-field">
      <span>API Base URL</span>
      <input v-model="settings.llm.baseUrl" placeholder="https://api.deepseek.com/v1" />
    </label>
    <!-- API Key + 验证（用户 2026-09-14）：验证按钮检查连通性并探测关闭思考的参数策略 -->
    <div class="set-field-row llm-row">
      <label class="set-field">
        <span>API Key</span>
        <input v-model="settings.llm.apiKey" type="password" placeholder="阶段1起存储于系统凭据库" />
      </label>
      <button class="set-button llm-verify" :disabled="settings.verifying" @click="settings.verifyLlm()">
        {{ settings.verifying ? "验证中…" : "验证" }}
      </button>
    </div>
    <!-- 模型：datalist 补全 + 手动拉取（打开设置自动尝试一次） -->
    <div class="set-field-row llm-row">
      <label class="set-field">
        <span>模型</span>
        <input v-model="settings.llm.model" list="llm-models" placeholder="deepseek-chat" />
        <datalist id="llm-models">
          <option v-for="m in settings.modelOptions" :key="m" :value="m" />
        </datalist>
      </label>
      <button class="set-button llm-verify" :disabled="settings.modelsFetching" @click="settings.fetchModels()">
        {{ settings.modelsFetching ? "获取中…" : "获取模型" }}
      </button>
    </div>
    <p class="set-hint">
      思考模式策略：{{ settings.llm.thinkingOff }}（验证时自动探测；none = 该端点无法关闭思考）。
    </p>
    <label class="set-field">
      <span>目标语言</span>
      <input v-model="settings.llm.targetLang" placeholder="zh（简体中文）" />
    </label>
    <label class="set-check">
      <input v-model="settings.llm.smartContext" type="checkbox" />
      <span>智能上下文翻译（跨页截断文本联合翻译）</span>
    </label>
    <div class="set-field">
      <span>日志</span>
      <pre class="log-box">{{ parse.llmLogs.join("\n") || "暂无日志" }}</pre>
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
</style>
