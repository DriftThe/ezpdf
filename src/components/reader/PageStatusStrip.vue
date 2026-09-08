<script setup lang="ts">
import { computed } from "vue";
import { useLibraryStore } from "../../stores/library";
import { useReaderStore } from "../../stores/reader";

/** 页级状态点阵：pending/处理中/done/failed 一眼可见，点击跳页 */
const lib = useLibraryStore();
const reader = useReaderStore();

const pages = computed(() => lib.currentBook?.pages ?? []);
const doneCount = computed(() => pages.value.filter((p) => p.status === "done").length);
const failedCount = computed(() => pages.value.filter((p) => p.status === "failed").length);

const emit = defineEmits<{ jump: [page: number] }>();

const statusText: Record<string, string> = {
  pending: "待解析",
  ocr_queued: "OCR 排队中",
  ocr_done: "OCR 完成",
  translating: "翻译中",
  done: "完成",
  failed: "失败",
};
</script>

<template>
  <footer v-if="lib.currentBook" class="strip">
    <span class="strip-label">页面</span>
    <div class="strip-dots">
      <button
        v-for="p in pages"
        :key="p.index"
        class="dot"
        :class="[p.status, { current: reader.currentPage === p.index + 1 }]"
        :title="`第 ${p.index + 1} 页 · ${statusText[p.status] ?? p.status}`"
        @click="emit('jump', p.index + 1)"
      />
    </div>
    <span class="strip-summary">
      完成 {{ doneCount }}/{{ pages.length }}<template v-if="failedCount"> · <b class="fail">失败 {{ failedCount }}</b></template>
    </span>
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
.dot.translating,
.dot.ocr_queued,
.dot.ocr_done {
  background: var(--accent);
  animation: dot-pulse 1.4s ease-in-out infinite;
}
.dot.failed {
  background: var(--err);
}
@keyframes dot-pulse {
  50% {
    opacity: 0.45;
  }
}
.strip-summary {
  font-size: 11px;
  color: var(--text-3);
  flex: none;
}
.fail {
  color: var(--err);
  font-weight: 600;
}
</style>
