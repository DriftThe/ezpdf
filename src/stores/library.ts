import { defineStore } from "pinia";
import { computed, ref } from "vue";
import type { Mode, PDF, PDFId, RepoGroup, RepoTree } from "../types/domain";
import { pdfIndexKey } from "../types/domain";
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
  const offlinePdfIds = ref<PDFId[]>([]);
  const currentPdfId = ref<PDFId | null>(null);
  /** 侧栏展开状态（打开 PDF 后自动收起为三条杠） */
  const sidebarOpen = ref(true);
  /** 全部已加载 PDF 实体，键 = pdfIndexKey（由后端 load_pdf 提供） */
  const pdfs = ref<Record<PDFId, PDF>>({});

  const currentPdf = computed<PDF | null>(() =>
    currentPdfId.value ? (pdfs.value[currentPdfId.value] ?? null) : null,
  );
  const offlinePdfs = computed<PDF[]>(() =>
    offlinePdfIds.value.map((id) => pdfs.value[id]).filter((p): p is PDF => !!p),
  );

  /**
   * 索引 → UI 分组视图：按 belong 归组（null = 根级），folders 里的空目录也占位；
   * 文件夹组按名排序，组内 PDF 按名排序。
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
    if (root?.length) groups.push({ folder: null, pdfs: [...root].sort(byName) });
    const folderNames = [...map.keys()]
      .filter((k): k is string => k !== null)
      .sort((a, b) => a.localeCompare(b, "zh"));
    for (const folder of folderNames) {
      groups.push({ folder, pdfs: (map.get(folder) ?? []).sort(byName) });
    }
    return groups;
  });

  function setMode(m: Mode): void {
    mode.value = m;
  }

  /**
   * 打开一份 PDF（两条入口合一，无嵌套）：
   * - 仓库树传索引项 PDFStruct（name/belong/bind）：未加载时凭索引 invoke 向后端查询；
   * - 离线列表/内部传索引键 PDFId（belong/name，根级为 name）：PDF 始终已加载。
   * 缓存策略：即用即丢——只保留当前 PDF 与离线列表，切换即释放旧实体。
   */
  async function selectPdf(target: PDFStruct | PDFId): Promise<void> {
    const isEntry = typeof target !== "string";
    const key = isEntry ? pdfIndexKey(target) : target;

    if (!pdfs.value[key]) {
      if (!isEntry) {
        // 传了索引键却未加载：异常状态（离线列表的 PDF 始终在 pdfs 里）
        toast("该 PDF 尚未解析或未加载，点击无效", "warn");
        return;
      }
      if (!repoRoot.value) {
        toast("尚未选择仓库，无法查询 PDF", "warn");
        return;
      }
      // ---- 阶段1 IPC：凭索引项向后端查询 PDF 实体（Rust 端实现 load_pdf 时对齐）----
      // 参数（JS camelCase / Rust snake_case）：root: 仓库根绝对路径；name: PDF 名；belong: string | null（null = 根级）
      // 返回：PDF（domain.ts 形状；前端忽略其 id，归一化为索引键 belong/name）
      // 约定：PDF 不在索引/读取失败 → throw（此处 toast）；bind 为 null 的未解析 PDF 建议返回空白页实体（可看原文，译文栏显示未解析）
      try {
        const pdf = await invoke<PDF>("load_pdf", {
          root: repoRoot.value,
          name: target.name,
          belong: target.belong,
        });
        pdfs.value[key] = { ...pdf, id: key };
      } catch (error) {
        toast(String(error), "error");
        return;
      }
    }

    currentPdfId.value = key;
    sidebarOpen.value = false; // 打开 PDF 后自动收起侧栏，把空间留给阅读器
    useReaderStore().restorePageFor(key);

    // 即用即丢：打开新 PDF 后只保留当前 PDF + 离线列表（结构 JSON 重读便宜；真正的内存大头在阶段2 pdfjs 层释放）
    for (const id of Object.keys(pdfs.value)) {
      if (id !== key && !offlinePdfIds.value.includes(id)) {
        delete pdfs.value[id];
      }
    }
  }

  function toggleSidebar(): void {
    sidebarOpen.value = !sidebarOpen.value;
  }

  function clearPdf(): void {
    currentPdfId.value = null;
    sidebarOpen.value = true;
  }

  // ---- 仓库：选择/刷新/加载（阶段1：导入与 PDF 目录 CRUD 后续接入） ----
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
    toast("阶段1接入：导入 PDF（拷入 PDF 目录并开始解析）");
  }
  function importPdfFolder(): void {
    toast("阶段1接入：导入 ezpdf PDF 文件夹");
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
    offlinePdfIds,
    currentPdfId,
    sidebarOpen,
    pdfs,
    currentPdf,
    offlinePdfs,
    setMode,
    selectPdf,
    toggleSidebar,
    clearPdf,
    chooseRepoRoot,
    importPdf,
    importPdfFolder,
    refreshRepo,
  };
});
