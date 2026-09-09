import { defineStore } from "pinia";
import { computed, ref } from "vue";
import type { Book, BookId, Mode, RepoNode, RepoTree } from "../types/domain";
import { toast } from "../composables/toast";
import { useReaderStore } from "./reader";
import { buildRepoNodes } from "../lib/repoTree";
import { buildMockLibrary, type MockLibrary } from "../mocks/mockData";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";

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
    const book = books.value[id];
    if (!book) {
      // 索引中的书未必已加载（未解析的 bind 为 null，或尚未通过 IPC 拉取实体）
      toast("该书尚未解析或未加载，点击无效", "warn");
      return;
    }
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

  // ---- 仓库：选择/刷新/加载（阶段1：导入与书目录 CRUD 后续接入） ----
  async function chooseRepoRoot(): Promise<void> {
    const repoPath = await open({
      directory: true,
      multiple: false,
      title: "选择仓库根目录",
    });
    if (repoPath === null) return;
    try {
      const created = await invoke<boolean>("check_and_build_repo", { root: repoPath });
      toast(created ? "已创建新仓库" : "已打开现有仓库");
      await loadRepo(repoPath);
    } catch (error) {
      toast(String(error), "error");
    }
  }
  function importPdf(): void {
    toast("阶段1接入：导入 PDF（拷入书目录并开始解析）");
  }
  function importBookFolder(): void {
    toast("阶段1接入：导入 ezpdf 书文件夹");
  }

  async function refreshRepo(): Promise<void> {
    if (!repoRoot.value) {
      toast("尚未选择仓库", "warn");
      return;
    }
    await loadRepo(repoRoot.value);
  }

  /** 拉取仓库树；成功才落地状态，失败保留原状并报错 */
  async function loadRepo(root: string): Promise<void> {
    try {
      const tree = await invoke<RepoTree>("gettree_from_config", { root });
      repoRoot.value = root;
      repoTree.value = buildRepoNodes(root, tree);
    } catch (error) {
      toast(String(error), "error");
    }
  }
  /** 骨架期演示数据（仅开发模式从侧栏触发）：扁平 RepoTree 走与真实 IPC 相同的转换路径 */
  function loadMock(data: MockLibrary = buildMockLibrary()): void {
    repoRoot.value = data.repoRoot;
    repoTree.value = buildRepoNodes(data.repoRoot, data.repoTree);
    offlineBookIds.value = data.offlineBookIds;
    books.value = data.books;
    if (!currentBookId.value && repoTree.value.length) {
      const firstBook = findFirstBook(repoTree.value);
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
