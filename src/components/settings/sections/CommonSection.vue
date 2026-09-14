<script setup lang="ts">
import { useSettingsStore } from "../../../stores/settings";

const settings = useSettingsStore();
</script>

<template>
  <div class="set-pane">
    <h2 class="set-title">常规</h2>
    <label class="set-check">
      <input v-model="settings.general.autoLaunch" type="checkbox" />
      <span>启动时自动唤醒 OCR 服务</span>
    </label>
    <p class="set-hint">
      开启后应用启动时先检查 Python/依赖/模型，全部就绪才拉起 pyserver（缺件时跳过，到「OCR 服务」页一键安装服务）。
    </p>
    <label class="set-check">
      <input v-model="settings.general.resumeOnStart" type="checkbox" />
      <span>启动时自动续跑未完成的解析</span>
    </label>
    <p class="set-hint">
      关闭时应用启动后处于暂停态（工具栏显示「启动翻译」），点一下才开始；只有同时开启「自动唤醒 OCR 服务」与「自动续跑」时，才会自动进入运行态（工具栏显示「暂停翻译」）。
    </p>
    <p class="set-hint">设置改动在退出设置页（左上角返回）时写入 config.json。</p>
    <!-- 更新检查（用户 2026-09-14）：启动静默一次，这里手动重查 -->
    <div class="set-field">
      <span>版本更新</span>
      <div class="set-field-row update-row">
        <span class="set-hint">v{{ settings.appVersion || "…" }} · {{ settings.updateText }}</span>
        <button class="set-button" :disabled="settings.updateBusy" @click="settings.checkUpdate(true)">
          {{ settings.updateBusy ? "检查中…" : "检查更新" }}
        </button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.update-row {
  gap: 10px;
  align-items: center;
}
</style>
