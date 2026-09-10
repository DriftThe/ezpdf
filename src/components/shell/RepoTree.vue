<script setup lang="ts">
import { computed, ref } from "vue";
import type { PDFStruct } from "../../../src-tauri/bindings/PDFStruct";
import { useLibraryStore } from "../../stores/library";

defineOptions({ name: "RepoTree" });

const lib = useLibraryStore();
const collapsed = ref<Set<string>>(new Set());

type Row = { kind: "folder"; name: string } | { kind: "pdf"; pdf: PDFStruct; inFolder: boolean };

/** 由分组索引派生可见行序列：目录行 + 其 PDF 行（可折叠）+ 根级 PDF 行（belong 为空，与目录平齐） */
const rows = computed<Row[]>(() => {
  const out: Row[] = [];
  for (const group of lib.repoGroups ?? []) {
    if (group.folder === null) {
      for (const pdf of group.pdfs) out.push({ kind: "pdf", pdf, inFolder: false });
    } else {
      out.push({ kind: "folder", name: group.folder });
      if (!collapsed.value.has(group.folder)) {
        for (const pdf of group.pdfs) out.push({ kind: "pdf", pdf, inFolder: true });
      }
    }
  }
  return out;
});

function toggle(folder: string): void {
  const s = new Set(collapsed.value);
  if (s.has(folder)) s.delete(folder);
  else s.add(folder);
  collapsed.value = s;
}
</script>

<template>
  <ul class="tree">
    <li v-for="row in rows" :key="row.kind === 'folder' ? `f:${row.name}` : row.pdf.id">
      <!-- 目录行（belong 分组）；悬浮时最右侧出现导入加号 -->
      <div v-if="row.kind === 'folder'" class="tree-row folder" @click="toggle(row.name)">
        <span class="chev" :class="{ open: !collapsed.has(row.name) }" aria-hidden="true">
          <svg viewBox="0 0 8 8" width="10" height="10"><path d="M2 1l4 3-4 3z" fill="currentColor" /></svg>
        </span>
        <span class="row-name folder-name">{{ row.name }}</span>
        <button class="row-add" title="导入 PDF 到此文件夹" :disabled="lib.importing" @click.stop="lib.importPdf(row.name)">
          <svg viewBox="0 0 10 10" width="20" height="20" aria-hidden="true">
            <path d="M5 1.2v7.6M1.2 5h7.6" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
          </svg>
        </button>
      </div>
      <!-- PDF 行：组内缩进；bind 为 null（未解析）半透明 -->
      <div
        v-else
        class="tree-row pdf"
        :class="{
          selected: lib.currentPdfId === row.pdf.id,
          unparsed: row.pdf.bind === null,
          nested: row.inFolder,
        }"
        :title="row.pdf.bind === null ? `${row.pdf.name}（未解析）` : row.pdf.id"
        @click="lib.selectPdf(row.pdf)"
      >
        <span class="pdf-glyph" aria-hidden="true">
          <svg viewBox="0 0 16 16" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.3">
            <path d="M4 2h6l2 2v10H4z" />
            <path d="M6 6.5h4M6 9h4M6 11.5h2.5" />
          </svg>
        </span>
        <span class="row-name pdf-name">{{ row.pdf.name }}</span>
      </div>
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
  gap: 7px;
  height: 34px;
  font-size: 17px;
  padding-right: 10px;
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
.tree-row.folder {
  padding-left: 6px;
}
.tree-row.pdf {
  padding-left: 10px;
}
.tree-row.pdf.nested {
  padding-left: 26px;
}
.chev {
  width: 16px;
  height: 16px;
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
  min-width: 0; /* 长名截断而非把右侧加号挤出行 */
  overflow: hidden;
  text-overflow: ellipsis;
}
.folder-name {
  font-weight: 600;
  color: var(--text-1);
}
.pdf-glyph {
  display: inline-flex;
  color: var(--text-3);
  flex: none;
}
.tree-row.selected .pdf-glyph {
  color: var(--accent);
}
.tree-row.unparsed {
  opacity: 0.55; /* 索引中 bind 为空：尚未解析 */
}
.tree-row.unparsed:hover {
  opacity: 1;
}
.row-add {
  margin-left: auto; /* 推到条目最右端 */
  width: 24px;
  height: 24px;
  flex: none;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  padding: 0;
  border: none;
  border-radius: var(--radius-sm);
  background: transparent;
  color: var(--text-3);
  cursor: pointer;
  opacity: 0; /* 默认隐藏，悬浮文件夹行时出现 */
}
.tree-row.folder:hover .row-add,
.row-add:focus-visible {
  opacity: 1;
}
.row-add:hover {
  background: var(--bg-hover);
  color: var(--accent);
}
/* 导入进行中：加号置灰失能，悬浮也不再高亮（需压过上面的 hover/浮现规则，故放最后） */
.row-add:disabled,
.tree-row.folder:hover .row-add:disabled {
  opacity: 0.45;
  background: transparent;
  color: var(--text-3);
  cursor: not-allowed;
}
</style>
