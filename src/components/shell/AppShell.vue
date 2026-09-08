<script setup lang="ts">
import Sidebar from "./Sidebar.vue";
import Toasts from "./Toasts.vue";
import ReaderToolbar from "../toolbar/ReaderToolbar.vue";
import ReaderArea from "../reader/ReaderArea.vue";
import PageStatusStrip from "../reader/PageStatusStrip.vue";
import SettingsModal from "../settings/SettingsModal.vue";

/**
 * App Shell 三区布局：
 * 左侧栏（仓库树/离线列表）│ 主区（工具栏 + 双栏阅读器 + 页状态条）│ 全局（设置/Toast）
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
    <SettingsModal />
    <Toasts />
  </div>
</template>

<style scoped>
.app-shell {
  height: 100%;
  display: flex;
  overflow: hidden;
}
.app-main {
  flex: 1;
  min-width: 0;
  display: flex;
  flex-direction: column;
}
</style>
