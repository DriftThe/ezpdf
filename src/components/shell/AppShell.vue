<script setup lang="ts">
import { onMounted } from "vue";
import Sidebar from "./Sidebar.vue";
import Toasts from "./Toasts.vue";
import TitleBar from "./TitleBar.vue";
import ConfirmDialog from "../common/ConfirmDialog.vue";
import ReaderToolbar from "../toolbar/ReaderToolbar.vue";
import ReaderArea from "../reader/ReaderArea.vue";
import PageStatusStrip from "../reader/PageStatusStrip.vue";
import SettingsPage from "../settings/SettingsPage.vue";
import { useSettingsStore } from "../../stores/settings";
import { useParseStore } from "../../stores/parse";
import { useLibraryStore } from "../../stores/library";

/**
 * App Shell：自绘标题栏（跨全宽）│ 左侧栏（仓库树）│ 主区（工具栏 + 双栏阅读器 + 页状态条）
 * │ 全局（设置整页/Toast/确认框）
 * 设置整页绝对定位覆盖 app-body（含页状态条与工具栏区域），顶部工具栏 z-index 更高保持可见；
 * 标题栏独立在外，始终可见（窗口装饰已关，装饰与拖动都由 TitleBar 自绘）。
 */
const settings = useSettingsStore();
const parse = useParseStore();
const lib = useLibraryStore();

// 启动序列（用户 2026-09-14）：读 config.json（覆盖 auth.cfg 默认）→ 默认置暂停
// （工具栏显示「启动翻译」；仅当「自动唤醒 OCR + 自动续跑」都开才自动运行）
// → 自动打开上次仓库 → 按开关自动唤醒 OCR 服务
onMounted(async () => {
  await settings.ensureLoaded();
  if (!(settings.general.autoLaunch && settings.general.resumeOnStart)) parse.paused = true;
  await lib.openLastRepo();
  void settings.checkUpdate(); // 启动静默检查更新（发现新版本才 toast）
  void parse.autoStartIfEnabled();
});
</script>

<template>
  <div class="app-shell">
    <TitleBar />
    <div class="app-body">
      <Sidebar />
      <main class="app-main">
        <ReaderToolbar />
        <ReaderArea />
        <PageStatusStrip />
      </main>
      <SettingsPage />
    </div>
    <Toasts />
    <ConfirmDialog />
  </div>
</template>

<style scoped>
.app-shell {
  height: 100%;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
/* 标题栏以下的工作区：设置整页覆盖层的定位基准 */
.app-body {
  flex: 1;
  min-height: 0;
  display: flex;
  position: relative;
}
.app-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
}
</style>
