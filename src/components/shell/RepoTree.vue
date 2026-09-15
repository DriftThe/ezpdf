<script setup lang="ts">
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import type { PDFStruct } from "../../../src-tauri/bindings/PDFStruct";
import { useLibraryStore } from "../../stores/library";
import { confirmDialog } from "../../composables/confirm";

defineOptions({ name: "RepoTree" });

const { t } = useI18n();
const lib = useLibraryStore();
const collapsed = ref<Set<string>>(new Set());
/** 打开的 PDF 行操作菜单（id）；点击树任意处关闭 */
const menuFor = ref<string | null>(null);

type FolderRow = { kind: "folder"; name: string; count: number };
type PdfRow = { kind: "pdf"; pdf: PDFStruct; inFolder: boolean; targets: string[] };

/** 由分组索引派生可见行序列：目录行 + 其 PDF 行（可折叠）+ 根级 PDF 行（belong 为空，与目录平齐）；
 *  每行的删除可用性（count）与移动目标（targets）在派生时一次算好，模板不再逐次过滤索引 */
const rows = computed<Array<FolderRow | PdfRow>>(() => {
  const index = lib.repoIndex;
  if (!index) return [];
  const moveTargets = (pdf: PDFStruct): string[] => index.folders.filter((f) => f !== pdf.belong);
  const out: Array<FolderRow | PdfRow> = [];
  for (const group of lib.repoGroups ?? []) {
    if (group.folder === null) {
      for (const pdf of group.pdfs) out.push({ kind: "pdf", pdf, inFolder: false, targets: moveTargets(pdf) });
    } else {
      out.push({ kind: "folder", name: group.folder, count: group.pdfs.length });
      if (!collapsed.value.has(group.folder)) {
        for (const pdf of group.pdfs) out.push({ kind: "pdf", pdf, inFolder: true, targets: moveTargets(pdf) });
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

function toggleMenu(id: string): void {
  menuFor.value = menuFor.value === id ? null : id;
}

/** 删除文件夹（自绘确认框，用户 2026-09-14）：非空时明示将连同其中 PDF 一并删除 */
async function onDeleteFolder(row: FolderRow): Promise<void> {
  const message =
    row.count > 0
      ? t("repo.confirmDeleteFolderWithPdfs", { name: row.name, n: row.count })
      : t("repo.confirmDeleteFolder", { name: row.name });
  if (await confirmDialog({ title: t("repo.deleteFolder"), message, confirmText: t("common.delete") })) {
    await lib.deleteFolder(row.name);
  }
}

async function onDeletePdf(pdf: PDFStruct): Promise<void> {
  menuFor.value = null;
  const ok = await confirmDialog({
    title: t("repo.deletePdf"),
    message: t("repo.confirmDeletePdf", { name: pdf.name }),
    confirmText: t("common.delete"),
  });
  if (ok) await lib.deletePdf(pdf);
}

async function onMove(pdf: PDFStruct, belong: string | null): Promise<void> {
  menuFor.value = null;
  await lib.movePdf(pdf.id, belong);
}

/** 清除解析状态（用户 2026-09-15）：丢弃 OCR 块与译文、重建空骨架后重新解析 */
async function onClearState(pdf: PDFStruct): Promise<void> {
  menuFor.value = null;
  const ok = await confirmDialog({
    title: t("repo.clearState"),
    message: t("repo.confirmClearState", { name: pdf.name }),
    confirmText: t("common.confirm"),
  });
  if (ok) await lib.clearPdfState(pdf);
}
</script>

<template>
  <ul class="tree" @click="menuFor = null">
    <li v-for="row in rows" :key="row.kind === 'folder' ? `f:${row.name}` : row.pdf.id">
      <!-- 目录行（belong 分组）；悬浮时最右侧出现导入加号与删除 -->
      <div v-if="row.kind === 'folder'" class="tree-row folder" @click="toggle(row.name)">
        <span class="chev" :class="{ open: !collapsed.has(row.name) }" aria-hidden="true">
          <svg viewBox="0 0 8 8" width="10" height="10"><path d="M2 1l4 3-4 3z" fill="currentColor" /></svg>
        </span>
        <span class="row-name folder-name">{{ row.name }}</span>
        <button class="row-btn" :title="t('repo.importToFolder')" :disabled="lib.importing" @click.stop="lib.importPdf(row.name)">
          <svg viewBox="0 0 10 10" width="20" height="20" aria-hidden="true">
            <path d="M5 1.2v7.6M1.2 5h7.6" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
          </svg>
        </button>
        <button
          class="row-btn"
          :title="row.count > 0 ? t('repo.deleteFolderWithPdfs') : t('repo.deleteFolder')"
          @click.stop="onDeleteFolder(row)"
        >
          <svg viewBox="0 0 14 14" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round">
            <path d="M2.5 3.5h9M5.5 3.5V2h3v1.5M3.5 3.5l.6 8h5.8l.6-8M6 6v3.5M8 6v3.5" />
          </svg>
        </button>
      </div>
      <!-- PDF 行：组内缩进；bind 为 null（未解析）半透明；悬浮出现移动/删除 -->
      <div
        v-else
        class="tree-row pdf"
        :class="{
          selected: lib.currentPdfId === row.pdf.id,
          unparsed: row.pdf.bind === null,
          nested: row.inFolder,
        }"
        :title="row.pdf.bind === null ? t('repo.unparsedName', { name: row.pdf.name }) : row.pdf.id"
        @click="lib.selectPdf(row.pdf)"
      >
        <span class="pdf-glyph" aria-hidden="true">
          <svg viewBox="0 0 16 16" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.3">
            <path d="M4 2h6l2 2v10H4z" />
            <path d="M6 6.5h4M6 9h4M6 11.5h2.5" />
          </svg>
        </span>
        <span class="row-name pdf-name">{{ row.pdf.name }}</span>
        <button class="row-btn" :title="t('repo.clearState')" @click.stop="onClearState(row.pdf)">
          <svg viewBox="0 0 16 16" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round">
            <path d="M13 8a5 5 0 1 1-1.6-3.7" />
            <path d="M13 2.5V5h-2.5" />
          </svg>
        </button>
        <button class="row-btn" :title="t('repo.moveToFolder')" @click.stop="toggleMenu(row.pdf.id)">
          <svg viewBox="0 0 16 16" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round">
            <path d="M2 4.5A1.5 1.5 0 0 1 3.5 3h3l1.5 2h4.5A1.5 1.5 0 0 1 14 6.5v5A1.5 1.5 0 0 1 12.5 13h-9A1.5 1.5 0 0 1 2 11.5z" />
            <path d="M6 9.5h4.5M8.5 7.5l2 2-2 2" />
          </svg>
        </button>
        <button class="row-btn" :title="t('repo.deleteFromRepo')" @click.stop="onDeletePdf(row.pdf)">
          <svg viewBox="0 0 14 14" width="17" height="17" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round">
            <path d="M2.5 3.5h9M5.5 3.5V2h3v1.5M3.5 3.5l.6 8h5.8l.6-8M6 6v3.5M8 6v3.5" />
          </svg>
        </button>
        <!-- 移动菜单：目标目录 + 移出到根 -->
        <div v-if="menuFor === row.pdf.id" class="row-menu" @click.stop>
          <button v-if="row.inFolder" class="menu-item" @click="onMove(row.pdf, null)">{{ t("repo.moveOut") }}</button>
          <div v-if="row.inFolder && row.targets.length > 0" class="menu-sep" />
          <button v-for="f in row.targets" :key="f" class="menu-item" @click="onMove(row.pdf, f)">
            {{ t("repo.moveInto", { folder: f }) }}
          </button>
          <div v-if="!row.inFolder && row.targets.length === 0" class="menu-empty">{{ t("repo.noOtherFolders") }}</div>
        </div>
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
  position: relative;
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
  min-width: 0; /* 长名截断而非把右侧按钮挤出行 */
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
.row-btn {
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
  opacity: 0; /* 默认隐藏，悬浮行时出现 */
}
.row-btn + .row-btn {
  margin-left: 0;
}
.tree-row:hover .row-btn,
.row-btn:focus-visible {
  opacity: 1;
}
.row-btn:hover {
  background: var(--bg-hover);
  color: var(--accent);
}
/* 导入/删除进行中或禁用：置灰失能，悬浮也不再高亮（需压过上面的 hover/浮现规则） */
.row-btn:disabled,
.tree-row:hover .row-btn:disabled {
  opacity: 0.45;
  background: transparent;
  color: var(--text-3);
  cursor: not-allowed;
}
/* ---- PDF 行移动菜单 ---- */
.row-menu {
  position: absolute;
  top: 30px;
  right: 8px;
  z-index: 30;
  min-width: 150px;
  max-width: 240px;
  padding: 4px;
  background: var(--bg-panel);
  border: 1px solid var(--border);
  border-radius: var(--radius-sm);
  box-shadow: var(--shadow-1);
  cursor: default;
}
.menu-item {
  display: block;
  width: 100%;
  border: none;
  background: transparent;
  text-align: left;
  padding: 5px 8px;
  border-radius: 5px;
  font-size: 13px;
  color: var(--text-2);
  cursor: pointer;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.menu-item:hover {
  background: var(--bg-hover);
  color: var(--text-1);
}
.menu-sep {
  height: 1px;
  margin: 4px 6px;
  background: var(--border);
}
.menu-empty {
  padding: 5px 8px;
  font-size: 12px;
  color: var(--text-3);
}
</style>
