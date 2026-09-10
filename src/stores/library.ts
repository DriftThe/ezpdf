import { defineStore } from "pinia";
import { computed, ref } from "vue";
import type { ImportOutcome, PDF, PDFId, RepoGroup, RepoTree } from "../types/domain";
import type { PDFStruct } from "../../src-tauri/bindings/PDFStruct";
import { toast } from "../composables/toast";
import { useReaderStore } from "./reader";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";

export const useLibraryStore = defineStore("library", () => {
  const repoRoot = ref<string | null>(null);
  /** .ezrepo 平铺索引（真相源）；树形呈现由 repoGroups 按 belong 派生，不做物理路径拼接 */
  const repoIndex = ref<RepoTree | null>(null);
  const currentPdfId = ref<PDFId | null>(null);
  /** 侧栏展开状态（打开 PDF 后自动收起为三条杠） */
  const sidebarOpen = ref(true);
  /** 全部已加载 PDF 实体，键 = 稳定 id（.ezrepo 条目 id） */
  const pdfs = ref<Record<PDFId, PDF>>({});

  const currentPdf = computed<PDF | null>(() =>
    currentPdfId.value ? (pdfs.value[currentPdfId.value] ?? null) : null,
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

  /**
   * 打开一份 PDF：凭稳定 id（.ezrepo 条目 id）。
   * 未加载时 invoke load_pdf 向后端查询；缓存策略：即用即丢——只保留当前 PDF。
   */
  async function selectPdf(pdf: PDFStruct): Promise<void> {
    const key = pdf.id;

    if (!pdfs.value[key]) {
      if (!repoRoot.value) {
        toast("尚未选择仓库，无法查询 PDF", "warn");
        return;
      }
      // ---- 阶段1 IPC：凭稳定 id 向后端查询 PDF 实体（Rust 端 load_pdf）----
      // 参数：root: 仓库根绝对路径；id: 稳定唯一标识符（.ezrepo 条目 id）
      // 返回：PDF 实体（id 应与请求一致；后端凭 id 查索引解析物理路径，前端不拼路径）
      // 约定：id 不在索引/读取失败 → throw（此处 toast）；bind 为 null 的未解析 PDF 建议返回空白实体（可看原文，译文栏显示未解析）
      try {
        const loaded = await invoke<PDF>("load_pdf", {
          root: repoRoot.value,
          id: pdf.id,
        });
        pdfs.value[key] = { ...loaded, id: key }; // 以请求的 id 归一化，保证实体 id 与存储键一致
      } catch (error) {
        toast(String(error), "error");
        return;
      }
    }

    currentPdfId.value = key;
    sidebarOpen.value = false; // 打开 PDF 后自动收起侧栏，把空间留给阅读器
    useReaderStore().restorePageFor(key);

    // 即用即丢：只保留当前 PDF（结构 JSON 重读便宜；真正的内存大头在阶段2 pdfjs 层释放）
    for (const id of Object.keys(pdfs.value)) {
      if (id !== key) {
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

  /** 导入进行中（后端多文件导入期间置 true，finally 保证释放）：
   *  置位后所有导入入口（侧栏按钮、文件夹加号）失能并显示"导入中"；importPdf 开头拦截重复触发 */
  const importing = ref(false);

  /**
   * 多文件导入：dialog 多选 PDF → 后端 copy 入库（name-id 命名）+ 写绑定 JSON 骨架 + 更新 .ezrepo
   * → 返回成败明细 → 前端拉取新索引刷新树。
   * @param belong 目标顶层目录；null = 仓库根级（侧栏/空态按钮入口）
   */
  async function importPdf(belong: string | null = null): Promise<void> {
    if (importing.value) {
      toast("导入中，请稍后", "warn");
      return;
    }
    if (!repoRoot.value) {
      toast("尚未选择仓库", "warn");
      return;
    }
    const picked = await open({
      multiple: true,
      directory: false,
      filters: [{ name: "PDF", extensions: ["pdf"] }],
    });
    if (!picked) return;
    const paths = Array.isArray(picked) ? picked : [picked];
    if (paths.length === 0) return;

    importing.value = true;
    try {
      const outcome = await invoke<ImportOutcome>("import_pdf", {
        root: repoRoot.value,
        belong,
        paths,
      });
      // 前端请求刷新仓库：gettree_from_config → repoIndex → repoGroups → 树自动更新
      await loadRepo(repoRoot.value);
      if (outcome.failed.length > 0) {
        const reasons = outcome.failed.map((f) => f.reason).join("；");
        toast(`已导入 ${outcome.imported.length} 份，失败 ${outcome.failed.length} 份：${reasons}`, "warn");
      } else {
        toast(`已导入 ${outcome.imported.length} 份 PDF`);
      }
    } catch (error) {
      toast(String(error), "error");
    } finally {
      importing.value = false;
    }
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
    repoRoot,
    repoGroups,
    currentPdfId,
    sidebarOpen,
    pdfs,
    currentPdf,
    selectPdf,
    toggleSidebar,
    clearPdf,
    chooseRepoRoot,
    importing,
    importPdf,
    importPdfFolder,
    refreshRepo,
  };
});
