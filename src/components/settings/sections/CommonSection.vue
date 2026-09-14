<script setup lang="ts">
import { useSettingsStore } from "../../../stores/settings";

const settings = useSettingsStore();
</script>

<template>
  <div class="set-pane">
    <h2 class="set-title">常规</h2>
    <label class="set-check">
      <input v-model="settings.ocr.autoLaunch" type="checkbox" />
      <span>启动时自动唤醒 OCR 服务</span>
    </label>
    <p class="set-hint">
      开启后应用启动时先检查 Python/依赖/模型，全部就绪才拉起 pyserver（缺环境或模型时跳过，到「OCR 服务」设置页处理）。
    </p>
    <p class="set-hint">设置改动在退出设置页（左上角返回）时写入 config.json。</p>
  </div>
</template>
