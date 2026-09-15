import { defineStore } from "pinia";
import { computed, ref } from "vue";
import type { ImportOutcome, PDF, PDFId, RepoGroup, RepoTree } from "../types/domain";
import type { PDFStruct } from "../../src-tauri/bindings/PDFStruct";
import { toast } from "../composables/toast";
import { t } from "../lib/i18n";
import { useReaderStore } from "./reader";
import { useParseStore } from "./parse";
import { useSettingsStore } from "./settings";
import { open } from "@tauri-apps/plugin-dialog";
import { invoke } from "@tauri-apps/api/core";

/** 导入结果文案：失败详情；仅页数未知则给原因；全部成功则简讯（措辞与原来一致） */
function formatImportOutcome(outcome: ImportOutcome): { text: string; kind: "warn" | "info" } {
  if (outcome.failed.length > 0) {
    const reasons = outcome.failed.map((f) => f.reason).join("；");
    const warn =
      outcome.warnings.length > 0
        ? t("library.importUnknownSuffix", { n: outcome.warnings.length })
        : "";
    return {
      text: t("library.importFailed", {
        n: outcome.imported.length,
        m: outcome.failed.length,
        reasons,
        warn,
      }),
      kind: "warn",
    };
  }
  if (outcome.warnings.length > 0) {
    return {
      text: t("library.importPartialUnknown", {
        n: outcome.imported.length,
        m: outcome.warnings.length,
        reasons: outcome.warnings.map((f) => f.reason).join("；"),
      }),
      kind: "warn",
    };
  }
  return { text: t("library.importDone", { n: outcome.imported.length }), kind: "info" };
}

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
        toast(t("library.noRepoForPdf"), "warn");
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
    useParseStore().wake(); // 打开书 = 调度事件：聚焦书自动开跑（含 Pending 书第一批）

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

  // ---- 仓库：选择/加载 + 条目增删改 ----
  async function chooseRepoRoot(): Promise<void> {
    const repoPath = await open({
      directory: true,
      multiple: false,
      title: t("library.chooseRepoTitle"),
    });
    if (repoPath === null) return;
    let created: boolean;
    try {
      created = await invoke<boolean>("check_and_build_repo", { root: repoPath });
    } catch (error) {
      toast(String(error), "error");
      return;
    }
    try {
      await loadRepo(repoPath);
      toast(t(created ? "library.repoCreated" : "library.repoOpened"));
    } catch (error) {
      toast(String(error), "error");
    }
  }

  /**
   * 启动自动打开上次仓库（用户 2026-09-14）：路径来自 config.json（settings.repoPath）。
   * 失败（目录被删/移动/索引损坏）→ 提示并清除持久化路径，回到未选择状态。
   */
  async function openLastRepo(): Promise<void> {
    const settings = useSettingsStore();
    const root = settings.repoPath;
    if (!root) return;
    try {
      await loadRepo(root);
    } catch (error) {
      toast(t("library.autoOpenFailed", { error: String(error) }), "warn");
      void settings.setRepoPath(null);
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
      toast(t("library.importingBusy"), "warn");
      return;
    }
    if (!repoRoot.value) {
      toast(t("library.noRepo"), "warn");
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
      if (outcome.imported.length > 0) {
        useParseStore().wake(); // 导入即开跑（用户拍板：新书自动进入调度，无需打开）
      }
      const report = formatImportOutcome(outcome);
      toast(report.text, report.kind);
    } catch (error) {
      toast(String(error), "error");
    } finally {
      importing.value = false;
    }
  }

  // ---- 仓库条目增删改（用户 2026-09-14）：文件夹是逻辑分组（belong），后端只改索引；
  //      成功后以后端返回的 RepoTree 就地更新树，无需整仓刷新 ----

  /** 仓库变更命令统一出口：成功就地更新索引；失败 toast 并返回 false */
  async function mutateRepoTree(command: string, args: Record<string, unknown>): Promise<boolean> {
    if (!repoRoot.value) return false;
    try {
      repoIndex.value = await invoke<RepoTree>(command, { root: repoRoot.value, ...args });
      return true;
    } catch (error) {
      toast(String(error), "error");
      return false;
    }
  }

  /** 新建文件夹（名由调用方输入；重名/非法名后端拒绝 → toast） */
  function createFolder(name: string): Promise<boolean> {
    if (!repoRoot.value) {
      toast(t("library.noRepo"), "warn");
      return Promise.resolve(false);
    }
    return mutateRepoTree("create_folder", { name });
  }

  /** 删除文件夹：后端级联删除其中 PDF（索引 + 库内文件） */
  function deleteFolder(name: string): Promise<boolean> {
    return mutateRepoTree("delete_folder", { name });
  }

  /** 删除 PDF：后端摘索引 + 删库内 PDF/绑定 JSON；若删的是当前打开的书 → 回空态 */
  async function deletePdf(pdf: PDFStruct): Promise<boolean> {
    const ok = await mutateRepoTree("delete_pdf", { id: pdf.id });
    if (ok) {
      delete pdfs.value[pdf.id];
      if (currentPdfId.value === pdf.id) currentPdfId.value = null;
    }
    return ok;
  }

  /** 移动 PDF：belong=目录名移入（不存在自动建组），null 移出到根级 */
  function movePdf(id: string, belong: string | null): Promise<boolean> {
    return mutateRepoTree("move_pdf", { id, belong });
  }

  /** 拉取仓库索引；成功才落地状态并持久化路径，失败保留原状（调用方负责提示） */
  async function loadRepo(root: string): Promise<void> {
    const index = await invoke<RepoTree>("gettree_from_config", { root });
    repoRoot.value = root;
    repoIndex.value = index; // 平铺索引直接落地，不做树转换
    void useSettingsStore().setRepoPath(root); // 下次启动默认打开（用户 2026-09-14）
    useParseStore().wake(); // 换仓/刷新 = 新的可处理书目，尝试续链
  }

  return {
    repoRoot,
    repoIndex,
    repoGroups,
    currentPdfId,
    sidebarOpen,
    pdfs,
    currentPdf,
    selectPdf,
    toggleSidebar,
    chooseRepoRoot,
    openLastRepo,
    importing,
    importPdf,
    loadRepo,
    createFolder,
    deleteFolder,
    deletePdf,
    movePdf,
  };
});
