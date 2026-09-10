<script setup lang="ts">
import { useLibraryStore } from "../../stores/library";
import { toast } from "../../composables/toast";
import RepoTree from "./RepoTree.vue";
import EmptyState from "../common/EmptyState.vue";

const lib = useLibraryStore();

function onOfflineTab(): void {
  toast("开发中");
}
</script>

<template>
  <aside class="sidebar" :class="{ closed: !lib.sidebarOpen }">
    <div class="side-inner">
    <div class="side-tabs">
      <button class="tab active">仓库</button>
      <button class="tab" @click="onOfflineTab">离线</button>
    </div>

    <div class="side-actions">
      <button class="btn primary grow" :disabled="lib.importing" @click="lib.importPdf()">{{ lib.importing ? "导入中" : "导入 PDF" }}</button>
      <button class="btn grow" @click="lib.refreshRepo">刷新仓库</button>
    </div>

    <div class="side-body">
      <RepoTree v-if="lib.repoRoot" />
      <EmptyState v-else title="未选择仓库" desc="选择一个文件夹作为 PDF 仓库">
        <button class="btn primary" @click="lib.chooseRepoRoot">选择仓库目录</button>
      </EmptyState>
    </div>

    <div class="side-footer">
      <div v-if="lib.repoRoot" class="repo-root" :title="lib.repoRoot">
        <span class="root-glyph" aria-hidden="true">
          <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.3">
            <path d="M2 4.5A1.5 1.5 0 0 1 3.5 3h3l1.5 2h4.5A1.5 1.5 0 0 1 14 6.5v5A1.5 1.5 0 0 1 12.5 13h-9A1.5 1.5 0 0 1 2 11.5z" />
          </svg>
        </span>
        <span class="root-path">{{ lib.repoRoot }}</span>
        <!-- <button class="btn ghost sm" @click="lib.refreshRepo">刷新</button> -->
      </div>
    </div>
    </div>
  </aside>
</template>

<style scoped>
.sidebar {
  width: var(--sidebar-w);
  flex: none;
  height: 100%;
  overflow: hidden; /* 收起时裁剪内容 */
  background: var(--bg-panel);
  border-right: 1px solid var(--border);
  transition: width 0.18s ease;
}
.sidebar.closed {
  width: 0;
  border-right-color: transparent;
}
.side-inner {
  width: var(--sidebar-w);
  height: 100%;
  display: flex;
  flex-direction: column;
}
.side-tabs {
  display: flex;
  padding: 8px 8px 0;
  gap: 2px;
}
.tab {
  flex: 1;
  border: none;
  background: transparent;
  padding: 6px 0;
  border-radius: var(--radius-sm) var(--radius-sm) 0 0;
  cursor: pointer;
  color: var(--text-2);
  font-weight: 600;
  border-bottom: 2px solid transparent;
}
.tab:hover {
  color: var(--text-1);
  background: var(--bg-hover);
}
.tab.active {
  color: var(--accent);
  border-bottom-color: var(--accent);
}
.side-actions {
  display: flex;
  gap: 6px;
  padding: 8px;
}
.grow {
  flex: 1;
}
.side-body {
  flex: 1;
  overflow: auto;
  padding: 2px 4px 8px;
}
.side-footer {
  border-top: 1px solid var(--border);
  padding: 6px 8px;
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.repo-root {
  display: flex;
  align-items: center;
  gap: 5px;
  font-size: 11px;
  color: var(--text-3);
  min-width: 0;
}
.root-glyph {
  display: inline-flex;
  flex: none;
}
.root-path {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  direction: rtl; /* 长路径时显示尾部 */
  text-align: left;
}
.dev-btn {
  color: var(--text-3);
}
</style>
