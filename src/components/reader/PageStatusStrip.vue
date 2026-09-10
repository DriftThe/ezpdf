<script setup lang="ts">
import { computed } from "vue";
import { useLibraryStore } from "../../stores/library";
import { useReaderStore } from "../../stores/reader";

/** 页级状态点阵：完成/待解析一眼可见，点击跳页 */
const lib = useLibraryStore();
const reader = useReaderStore();

const pages = computed(() => lib.currentPdf?.pages ?? []);
const doneCount = computed(() => pages.value.filter((p) => p.finished).length);

const emit = defineEmits<{ jump: [page: number] }>();
</script>

<template>
  <footer v-if="lib.currentPdf" class="strip">
    <span class="strip-label">页面</span>
    <div class="strip-dots">
      <button
        v-for="p in pages"
        :key="p.index"
        class="dot"
        :class="[p.finished ? 'done' : '', { current: reader.currentPage === p.index }]"
        :title="`第 ${p.index} 页 · ${p.finished ? '完成' : '待解析'}`"
        @click="emit('jump', p.index)"
      />
    </div>
    <span class="strip-summary">完成 {{ doneCount }}/{{ pages.length }}</span>
  </footer>
</template>

<style scoped>
.strip {
  height: 34px;
  flex: none;
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 0 12px;
  background: var(--bg-panel);
  border-top: 1px solid var(--border);
  user-select: none;
}
.strip-label {
  font-size: 11px;
  color: var(--text-3);
  flex: none;
}
.strip-dots {
  flex: 1;
  display: flex;
  align-items: center;
  gap: 3px;
  overflow-x: auto;
  padding: 4px 2px;
  scrollbar-width: thin;
}
.dot {
  width: 10px;
  height: 10px;
  flex: none;
  border-radius: 3px;
  border: none;
  padding: 0;
  cursor: pointer;
  background: var(--pending);
}
.dot:hover {
  outline: 1px solid var(--text-3);
}
.dot.current {
  outline: 2px solid var(--accent);
  outline-offset: 1px;
}
.dot.done {
  background: var(--ok);
}
.strip-summary {
  font-size: 11px;
  color: var(--text-3);
  flex: none;
}
</style>
