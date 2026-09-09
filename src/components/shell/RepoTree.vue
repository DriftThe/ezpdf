<script setup lang="ts">
import { computed, ref } from "vue";
import type { PDFStruct } from "../../../src-tauri/bindings/PDFStruct";
import { pdfIndexKey } from "../../types/domain";
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
    <li v-for="row in rows" :key="row.kind === 'folder' ? `f:${row.name}` : pdfIndexKey(row.pdf)">
      <!-- 目录行（belong 分组） -->
      <div v-if="row.kind === 'folder'" class="tree-row folder" @click="toggle(row.name)">
        <span class="chev" :class="{ open: !collapsed.has(row.name) }" aria-hidden="true">
          <svg viewBox="0 0 8 8" width="8" height="8"><path d="M2 1l4 3-4 3z" fill="currentColor" /></svg>
        </span>
        <span class="row-name folder-name">{{ row.name }}</span>
      </div>
      <!-- PDF 行：组内缩进；bind 为 null（未解析）半透明 -->
      <div
        v-else
        class="tree-row pdf"
        :class="{
          selected: lib.currentPdfId === pdfIndexKey(row.pdf),
          unparsed: row.pdf.bind === null,
          nested: row.inFolder,
        }"
        :title="row.pdf.bind === null ? `${row.pdf.name}（未解析）` : pdfIndexKey(row.pdf)"
        @click="lib.selectPdf(row.pdf)"
      >
        <span class="pdf-glyph" aria-hidden="true">
          <svg viewBox="0 0 16 16" width="13" height="13" fill="none" stroke="currentColor" stroke-width="1.3">
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
.tree-row.folder {
  padding-left: 4px;
}
.tree-row.pdf {
  padding-left: 8px;
}
.tree-row.pdf.nested {
  padding-left: 20px;
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
</style>
