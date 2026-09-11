<script setup lang="ts">
import { computed } from "vue";
import { useLibraryStore } from "../../stores/library";
import { useReaderStore } from "../../stores/reader";

/** 页级状态点阵：pdfjs 实测页数为总量，完成态按 1-based 页号从绑定 JSON 查表 */
const lib = useLibraryStore();
const reader = useReaderStore();

const total = computed(() => reader.pageCount);
const finishedSet = computed(() => {
  const s = new Set<number>();
  for (const p of lib.currentPdf?.pages ?? []) {
    if (p.finished) s.add(p.index);
  }
  return s;
});
const doneCount = computed(() => finishedSet.value.size);
</script>

<template>
  <footer v-if="lib.currentPdf" class="strip">
    <span class="strip-label">页面</span>
    <div class="strip-dots">
      <button
        v-for="n in total"
        :key="n"
        class="dot"
        :class="[finishedSet.has(n) ? 'done' : '', { current: reader.currentPage === n }]"
        :title="`第 ${n} 页 · ${finishedSet.has(n) ? '完成' : '待解析'}`"
        @click="reader.gotoPage(n)"
      />
    </div>
    <span class="strip-summary">完成 {{ doneCount }}/{{ total }}</span>
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
