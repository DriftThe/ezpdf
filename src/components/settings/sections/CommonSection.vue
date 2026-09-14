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
  </div>
</template>
