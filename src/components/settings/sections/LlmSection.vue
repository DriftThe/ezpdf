<script setup lang="ts">
import { useParseStore } from "../../../stores/parse";
import { useSettingsStore } from "../../../stores/settings";

const settings = useSettingsStore();
const parse = useParseStore();
</script>

<template>
  <div class="set-pane">
    <h2 class="set-title">LLM 翻译</h2>
    <label class="set-field">
      <span>API Base URL</span>
      <input v-model="settings.llm.baseUrl" placeholder="https://api.deepseek.com/v1" />
    </label>
    <label class="set-field">
      <span>API Key</span>
      <input v-model="settings.llm.apiKey" type="password" placeholder="阶段1起存储于系统凭据库" />
    </label>
    <label class="set-field">
      <span>模型</span>
      <input v-model="settings.llm.model" placeholder="deepseek-chat" />
    </label>
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
