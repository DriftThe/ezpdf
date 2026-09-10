<script setup lang="ts">
import { useSettingsStore } from "../../../stores/settings";

const settings = useSettingsStore();
</script>

<template>
  <div class="set-pane">
    <h2 class="set-title">解析</h2>
    <div class="set-field-row">
      <label class="set-field">
        <span>OCR 并发</span>
        <input v-model.number="settings.parse.ocrConcurrency" type="number" min="1" max="4" />
      </label>
      <label class="set-field">
        <span>翻译并发</span>
        <input v-model.number="settings.parse.llmConcurrency" type="number" min="1" max="8" />
      </label>
    </div>
    <label class="set-check">
      <input v-model="settings.parse.resumeOnStart" type="checkbox" />
      <span>启动时自动续跑未完成的解析</span>
    </label>
  </div>
</template>
