<script setup lang="ts">
import { ref } from "vue";
import type { RepoNode } from "../../types/domain";
import { useLibraryStore } from "../../stores/library";

defineOptions({ name: "RepoTree" });

const props = defineProps<{
  nodes: RepoNode[];
  depth?: number;
}>();

const lib = useLibraryStore();
const collapsed = ref<Set<string>>(new Set());

function toggle(path: string): void {
  const s = new Set(collapsed.value);
  if (s.has(path)) s.delete(path);
  else s.add(path);
  collapsed.value = s;
}

function indent(): string {
  return `${(props.depth ?? 0) * 12}px`;
}
</script>

<template>
  <ul class="tree">
    <li v-for="node in nodes" :key="node.path">
      <!-- 文件夹 -->
      <div v-if="node.type === 'folder'" class="tree-row folder" :style="{ paddingLeft: indent() }" @click="toggle(node.path)">
        <span class="chev" :class="{ open: !collapsed.has(node.path) }" aria-hidden="true">
          <svg viewBox="0 0 8 8" width="8" height="8"><path d="M2 1l4 3-4 3z" fill="currentColor" /></svg>
        </span>
        <span class="row-name folder-name">{{ node.name }}</span>
      </div>
      <!-- 书 -->
      <div
        v-else
        class="tree-row book"
        :class="{ selected: lib.currentBookId === node.path }"
        :style="{ paddingLeft: `calc(${indent()} + 4px)` }"
        :title="node.path"
        @click="lib.selectBook(node.path)"
      >
        <span class="book-glyph" aria-hidden="true">
          <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.3">
            <path d="M4 2h6l2 2v10H4z" />
            <path d="M6 6.5h4M6 9h4M6 11.5h2.5" />
          </svg>
        </span>
        <span class="row-name book-name">{{ node.name }}</span>
      </div>
      <RepoTree v-if="node.type === 'folder' && !collapsed.has(node.path)" :nodes="node.children" :depth="(props.depth ?? 0) + 1" />
    </li>
  </ul>
</template>

<style scoped>
.tree {
  list-style: none;
  margin: 0;
  padding: 0;
}
.tree-row {
  display: flex;
  align-items: center;
  gap: 5px;
  height: 26px;
  padding-right: 8px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  color: var(--text-2);
  user-select: none;
  white-space: nowrap;
}
.tree-row:hover {
  background: var(--bg-hover);
}
.tree-row.selected {
  background: var(--accent-weak);
  color: var(--accent);
}
.chev {
  width: 12px;
  height: 12px;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: var(--text-3);
  transition: transform 0.12s;
  flex: none;
}
.chev.open {
  transform: rotate(90deg);
}
.row-name {
  overflow: hidden;
  text-overflow: ellipsis;
}
.folder-name {
  font-weight: 600;
  color: var(--text-1);
}
.book-glyph {
  display: inline-flex;
  color: var(--text-3);
  flex: none;
}
.tree-row.selected .book-glyph {
  color: var(--accent);
}
</style>
