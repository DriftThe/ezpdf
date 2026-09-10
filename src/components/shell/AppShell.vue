<script setup lang="ts">
import Sidebar from "./Sidebar.vue";
import Toasts from "./Toasts.vue";
import ReaderToolbar from "../toolbar/ReaderToolbar.vue";
import ReaderArea from "../reader/ReaderArea.vue";
import PageStatusStrip from "../reader/PageStatusStrip.vue";
import SettingsPage from "../settings/SettingsPage.vue";

/**
 * App Shell 三区布局：
 * 左侧栏（仓库树/离线列表）│ 主区（工具栏 + 双栏阅读器 + 页状态条）│ 全局（设置整页/Toast）
 * 设置整页绝对定位覆盖 sidebar+reader（含页状态条），顶部工具栏 z-index 更高保持可见。
 */
</script>

<template>
  <div class="app-shell">
    <Sidebar />
    <main class="app-main">
      <ReaderToolbar />
      <ReaderArea />
      <PageStatusStrip />
    </main>
    <SettingsPage />
    <Toasts />
  </div>
</template>

<style scoped>
.app-shell {
  height: 100%;
  display: flex;
  overflow: hidden;
  position: relative; /* 设置整页覆盖层的定位基准 */
}
.app-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
}
</style>
