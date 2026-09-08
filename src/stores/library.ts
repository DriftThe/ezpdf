import { defineStore } from "pinia";
import { computed, ref } from "vue";
import type { Book, BookId, Mode, RepoNode } from "../types/domain";
import { toast } from "../composables/toast";
import { useReaderStore } from "./reader";
import { buildMockLibrary, type MockLibrary } from "../mocks/mockData";

export const useLibraryStore = defineStore("library", () => {
  const mode = ref<Mode>("repo");
  const repoRoot = ref<string | null>(null);
  const repoTree = ref<RepoNode[]>([]);
  const offlineBookIds = ref<BookId[]>([]);
  const currentBookId = ref<BookId | null>(null);
  /** 侧栏展开状态（打开书籍后自动收起为三条杠） */
  const sidebarOpen = ref(true);
  /** 全部已加载书籍（阶段1起由 Rust 端提供） */
  const books = ref<Record<BookId, Book>>({});

  const currentBook = computed<Book | null>(() =>
    currentBookId.value ? (books.value[currentBookId.value] ?? null) : null,
  );
  const offlineBooks = computed<Book[]>(() =>
    offlineBookIds.value.map((id) => books.value[id]).filter((b): b is Book => !!b),
  );

  function setMode(m: Mode): void {
    mode.value = m;
  }

  function selectBook(id: BookId): void {
    if (!books.value[id]) return;
    currentBookId.value = id;
    sidebarOpen.value = false; // 打开书后自动收起侧栏，把空间留给阅读器
    useReaderStore().restorePageFor(id);
  }

  function toggleSidebar(): void {
    sidebarOpen.value = !sidebarOpen.value;
  }

  function clearBook(): void {
    currentBookId.value = null;
    sidebarOpen.value = true;
  }

  // ---- 以下动作在阶段1/3/4 接入 Tauri IPC ----
  function chooseRepoRoot(): void {
    toast("阶段1接入：选择仓库根目录");
  }
  function importPdf(): void {
    toast("阶段1接入：导入 PDF（拷入书目录并开始解析）");
  }
  function importBookFolder(): void {
    toast("阶段1接入：导入 ezpdf 书文件夹");
  }
  function refreshRepo(): void {
    toast("阶段1接入：重新扫描仓库");
  }

  /** 骨架期演示数据（仅开发模式从侧栏触发） */
  function loadMock(data: MockLibrary = buildMockLibrary()): void {
    repoRoot.value = data.repoRoot;
    repoTree.value = data.repoTree;
    offlineBookIds.value = data.offlineBookIds;
    books.value = data.books;
    if (!currentBookId.value && data.repoTree.length) {
      const firstBook = findFirstBook(data.repoTree);
      if (firstBook) selectBook(firstBook);
    }
    toast("已加载演示数据");
  }

  return {
    mode,
    repoRoot,
    repoTree,
    offlineBookIds,
    currentBookId,
    sidebarOpen,
    books,
    currentBook,
    offlineBooks,
    setMode,
    selectBook,
    toggleSidebar,
    clearBook,
    chooseRepoRoot,
    importPdf,
    importBookFolder,
    refreshRepo,
    loadMock,
  };
});

function findFirstBook(nodes: RepoNode[]): BookId | null {
  for (const n of nodes) {
    if (n.type === "book") return n.path;
    const found = findFirstBook(n.children);
    if (found) return found;
  }
  return null;
}
