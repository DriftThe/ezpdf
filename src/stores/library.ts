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
  /** Flat index (source of truth); repoGroups derives the tree. */
  const repoIndex = ref<RepoTree | null>(null);
  const currentPdfId = ref<PDFId | null>(null);
  const sidebarOpen = ref(true);
  const pdfs = ref<Record<PDFId, PDF>>({});

  const currentPdf = computed<PDF | null>(() =>
    currentPdfId.value ? (pdfs.value[currentPdfId.value] ?? null) : null,
  );

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

  /** Cache a PDF by id; the backend resolves the path (frontend never builds it). */
  async function loadPdf(id: string): Promise<PDF> {
    const loaded = await invoke<PDF>("load_pdf", { root: repoRoot.value, id });
    const normalized: PDF = { ...loaded, id };
    pdfs.value[id] = normalized;
    return normalized;
  }

  /** Open a PDF and keep only the current one cached (re-reading the JSON is cheap). */
  async function selectPdf(pdf: PDFStruct): Promise<void> {
    const key = pdf.id;

    if (!pdfs.value[key]) {
      if (!repoRoot.value) {
        toast(t("library.noRepoForPdf"), "warn");
        return;
      }
      try {
        await loadPdf(key);
      } catch (error) {
        toast(String(error), "error");
        return;
      }
    }

    currentPdfId.value = key;
    sidebarOpen.value = false;
    useReaderStore().restorePageFor(key);
    useParseStore().wake();

    for (const id of Object.keys(pdfs.value)) {
      if (id !== key) {
        delete pdfs.value[id];
      }
    }
  }

  function toggleSidebar(): void {
    sidebarOpen.value = !sidebarOpen.value;
  }

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

  const importing = ref(false);

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
      await loadRepo(repoRoot.value);
      if (outcome.imported.length > 0) {
        useParseStore().wake();
      }
      const report = formatImportOutcome(outcome);
      toast(report.text, report.kind);
    } catch (error) {
      toast(String(error), "error");
    } finally {
      importing.value = false;
    }
  }

  /** Update the index in place on success, else toast + false. */
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

  function createFolder(name: string): Promise<boolean> {
    if (!repoRoot.value) {
      toast(t("library.noRepo"), "warn");
      return Promise.resolve(false);
    }
    return mutateRepoTree("create_folder", { name });
  }

  function deleteFolder(name: string): Promise<boolean> {
    return mutateRepoTree("delete_folder", { name });
  }

  async function deletePdf(pdf: PDFStruct): Promise<boolean> {
    const ok = await mutateRepoTree("delete_pdf", { id: pdf.id });
    if (ok) {
      delete pdfs.value[pdf.id];
      if (currentPdfId.value === pdf.id) currentPdfId.value = null;
    }
    return ok;
  }

  function movePdf(id: string, belong: string | null): Promise<boolean> {
    return mutateRepoTree("move_pdf", { id, belong });
  }

  /** Rebuild the empty 1..=N skeleton; total = pdfjs count for the open book, else 0. */
  async function clearPdfState(pdf: PDFStruct): Promise<boolean> {
    if (!repoRoot.value) return false;
    const total = currentPdfId.value === pdf.id ? useReaderStore().pageCount : 0;
    try {
      await invoke("reset_pdf_state", { root: repoRoot.value, id: pdf.id, total });
    } catch (error) {
      toast(t("library.clearFailed", { err: String(error) }), "error");
      return false;
    }
    delete pdfs.value[pdf.id];
    if (currentPdfId.value === pdf.id) await selectPdf(pdf);
    toast(t("library.clearDone", { name: pdf.name }), "info");
    useParseStore().wake(); // reset state = new work available
    return true;
  }

  async function loadRepo(root: string): Promise<void> {
    const index = await invoke<RepoTree>("gettree_from_config", { root });
    repoRoot.value = root;
    repoIndex.value = index;
    void useSettingsStore().setRepoPath(root);
    useParseStore().wake();
  }

  return {
    repoRoot,
    repoIndex,
    repoGroups,
    currentPdfId,
    sidebarOpen,
    pdfs,
    currentPdf,
    loadPdf,
    selectPdf,
    toggleSidebar,
    chooseRepoRoot,
    openLastRepo,
    importing,
    importPdf,
    createFolder,
    deleteFolder,
    deletePdf,
    movePdf,
    clearPdfState,
  };
});
