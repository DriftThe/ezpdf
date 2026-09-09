<script setup lang="ts">
import { useLibraryStore } from "../../stores/library";

const lib = useLibraryStore();
</script>

<template>
  <ul class="offline-list">
    <li
      v-for="p in lib.offlinePdfs"
      :key="p.id"
      class="offline-item"
      :class="{ selected: lib.currentPdfId === p.id }"
      :title="p.id"
      @click="lib.selectPdf(p.id)"
    >
      <div class="item-name">{{ p.name }}</div>
      <div class="item-sub">{{ p.meta.pageCount }} 页 · {{ p.meta.llmModel }}</div>
    </li>
  </ul>
</template>

<style scoped>
.offline-list {
  list-style: none;
  margin: 0;
  padding: 4px;
}
.offline-item {
  padding: 7px 10px;
  border-radius: var(--radius-sm);
  cursor: pointer;
  user-select: none;
}
.offline-item:hover {
  background: var(--bg-hover);
}
.offline-item.selected {
  background: var(--accent-weak);
}
.offline-item.selected .item-name {
  color: var(--accent);
}
.item-name {
  font-weight: 600;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.item-sub {
  font-size: 11px;
  color: var(--text-3);
}
</style>
