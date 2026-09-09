import { defineStore } from "pinia";
import { computed, ref } from "vue";
import type { Book, BookId, Mode, RepoGroup, RepoTree } from "../types/domain";
import { bookIndexKey } from "../types/domain";
import type { PDFStruct } from "../../src-tauri/bindings/PDFStruct";
import { toast } from "../composables/toast";
import { useReaderStore } from "./reader";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";

export const useLibraryStore = defineStore("library", () => {
  const mode = ref<Mode>("repo");
  const repoRoot = ref<string | null>(null);
  /** .ezrepo 平铺索引（真相源）；树形呈现由 repoGroups 按 belong 派生，不做物理路径拼接 */
  const repoIndex = ref<RepoTree | null>(null);
  const offlineBookIds = ref<BookId[]>([]);
  const currentBookId = ref<BookId | null>(null);
  /** 侧栏展开状态（打开书籍后自动收起为三条杠） */
  const sidebarOpen = ref(true);
  /** 全部已加载书籍，键 = bookIndexKey（阶段1起由 Rust 端提供） */
  const books = ref<Record<BookId, Book>>({});

  const currentBook = computed<Book | null>(() =>
    currentBookId.value ? (books.value[currentBookId.value] ?? null) : null,
  );
  const offlineBooks = computed<Book[]>(() =>
    offlineBookIds.value.map((id) => books.value[id]).filter((b): b is Book => !!b),
  );

  /**
   * 索引 → UI 分组视图：按 belong 归组（null = 根级），folders 里的空目录也占位；
   * 文件夹组按名排序，组内书按名排序。
   */
  const repoGroups = computed<RepoGroup[] | null>(() => {
    const idx = repoIndex.value;
    if (!idx) return null;
    const map = new Map<string | null, PDFStruct[]>();
    for (const pdf of idx.pdfs) {
      const key = pdf.belong?.trim() || null;
      const list = map.get(key);
      if (list) list.push(pdf);
      else map.set(key, [pdf]);
    }
    for (const folder of idx.folders) {
      const key = folder.trim() || null;
      if (!map.has(key)) map.set(key, []);
    }
    const byName = (a: PDFStruct, b: PDFStruct) => a.name.localeCompare(b.name, "zh");
    const groups: RepoGroup[] = [];
    const root = map.get(null);
    if (root?.length) groups.push({ folder: null, books: [...root].sort(byName) });
    const folderNames = [...map.keys()]
      .filter((k): k is string => k !== null)
      .sort((a, b) => a.localeCompare(b, "zh"));
    for (const folder of folderNames) {
      groups.push({ folder, books: (map.get(folder) ?? []).sort(byName) });
    }
    return groups;
  });

  function setMode(m: Mode): void {
    mode.value = m;
  }

  /**
   * 打开一本书（两条入口合一，无嵌套）：
   * - 仓库树传索引项 PDFStruct（name/belong/bind）：未加载时凭索引 invoke 向后端查询；
   * - 离线书架/内部传索引键 BookId（belong/name，根级为 name）：书始终已加载。
   * 缓存策略：即用即丢——只保留当前书与离线书架，切书即释放旧实体。
   */
  async function selectBook(target: PDFStruct | BookId): Promise<void> {
    const isEntry = typeof target !== "string";
    const key = isEntry ? bookIndexKey(target) : target;

    if (!books.value[key]) {
      if (!isEntry) {
        // 传了索引键却未加载：异常状态（离线书架的书始终在 books 里）
        toast("该书尚未解析或未加载，点击无效", "warn");
        return;
      }
      if (!repoRoot.value) {
        toast("尚未选择仓库，无法查询书籍", "warn");
        return;
      }
      // ---- 阶段1 IPC：凭索引项向后端查询书实体（Rust 端实现 load_book 时对齐）----
      // 参数（JS camelCase / Rust snake_case）：root: 仓库根绝对路径；name: 书名；belong: string | null（null = 根级）
      // 返回：Book（domain.ts 形状；前端忽略其 id，归一化为索引键 belong/name）
      // 约定：书不在索引/PDF 读取失败 → throw（此处 toast）；bind 为 null 的未解析书建议返回空白页实体（可看原文，译文栏显示未解析）
      try {
        const book = await invoke<Book>("load_book", {
          root: repoRoot.value,
          name: target.name,
          belong: target.belong,
        });
        books.value[key] = { ...book, id: key };
      } catch (error) {
        toast(String(error), "error");
        return;
      }
    }

    currentBookId.value = key;
    sidebarOpen.value = false; // 打开书后自动收起侧栏，把空间留给阅读器
    useReaderStore().restorePageFor(key);

    // 即用即丢：开新书后只保留当前书 + 离线书架（结构 JSON 重读便宜；真正的内存大头在阶段2 pdfjs 层释放）
    for (const id of Object.keys(books.value)) {
      if (id !== key && !offlineBookIds.value.includes(id)) {
        delete books.value[id];
      }
    }
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
    // if (!repoRoot.value) {
    //   toast("尚未选择仓库", "warn");
    //   return;
    // }
    // await loadRepo(repoRoot.value);
  }

  /** 拉取仓库索引；成功才落地状态，失败保留原状并报错 */
  async function loadRepo(root: string): Promise<void> {
    try {
      const index = await invoke<RepoTree>("gettree_from_config", { root });
      repoRoot.value = root;
      repoIndex.value = index; // 平铺索引直接落地，不做树转换
    } catch (error) {
      toast(String(error), "error");
    }
  }
  return {
    mode,
    repoRoot,
    repoGroups,
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
  };
});
