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

/** Import result text: failure details, unknown-page reasons, or a success note. */
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
  /** Flat .ezrepo index (source of truth); repoGroups derives the tree by belong. */
  const repoIndex = ref<RepoTree | null>(null);
  const currentPdfId = ref<PDFId | null>(null);
  /** Sidebar open state (auto-collapses after opening a PDF). */
  const sidebarOpen = ref(true);
  /** Loaded PDF entities keyed by stable id. */
  const pdfs = ref<Record<PDFId, PDF>>({});

  const currentPdf = computed<PDF | null>(() =>
    currentPdfId.value ? (pdfs.value[currentPdfId.value] ?? null) : null,
  );

  /** Index → grouped view by belong (null = root); empty folders still get a group.
   *  Folders and PDFs within each group are sorted by name. */
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

  /** Fetch a PDF by id into the cache (the only load_pdf call site).
   *  Normalizes id to the request key; the backend resolves the path, the frontend never builds it.
   *  Throws on failure; background books read directly instead (parse.bookState). */
  async function loadPdf(id: string): Promise<PDF> {
    const loaded = await invoke<PDF>("load_pdf", { root: repoRoot.value, id });
    const normalized: PDF = { ...loaded, id };
    pdfs.value[id] = normalized;
    return normalized;
  }

  /** Open a PDF by stable id; cached use-and-discard — only the current PDF is kept. */
  async function selectPdf(pdf: PDFStruct): Promise<void> {
    const key = pdf.id;

    if (!pdfs.value[key]) {
      if (!repoRoot.value) {
        toast(t("library.noRepoForPdf"), "warn");
        return;
      }
      try {
        await loadPdf(key); // id missing / read failure → toast and don't switch
      } catch (error) {
        toast(String(error), "error");
        return;
      }
    }

    currentPdfId.value = key;
    sidebarOpen.value = false; // collapse the sidebar to give the reader room
    useReaderStore().restorePageFor(key);
    useParseStore().wake(); // opening a book wakes the scheduler (incl. a first batch for Pending)

    // use-and-discard: keep only the current PDF (re-reading the JSON is cheap)
    for (const id of Object.keys(pdfs.value)) {
      if (id !== key) {
        delete pdfs.value[id];
      }
    }
  }

  function toggleSidebar(): void {
    sidebarOpen.value = !sidebarOpen.value;
  }

  // ---- repo: select/load + entry create/delete/move ----
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

  /** Auto-open the last repo from settings.repoPath; on failure clear it and go unselected. */
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

  /** Import in progress; disables all import entries and blocks re-entry. */
  const importing = ref(false);

  /** Multi-file import: dialog → backend copy (name-id) + skeleton JSON + .ezrepo update
   *  → outcome details → reload the index. `belong` null = repo root. */
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
      // reload the index: gettree_from_config → repoIndex → repoGroups → the tree updates
      await loadRepo(repoRoot.value);
      if (outcome.imported.length > 0) {
        useParseStore().wake(); // imported books enter the scheduler without being opened
      }
      const report = formatImportOutcome(outcome);
      toast(report.text, report.kind);
    } catch (error) {
      toast(String(error), "error");
    } finally {
      importing.value = false;
    }
  }

  // ---- repo entry mutations: folders are logical (belong); the returned RepoTree
  //      updates the tree in place, no full refresh ----

  /** Single mutation entry point: update the index in place on success, else toast + false. */
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

  /** Create a folder (backend rejects duplicates/invalid names). */
  function createFolder(name: string): Promise<boolean> {
    if (!repoRoot.value) {
      toast(t("library.noRepo"), "warn");
      return Promise.resolve(false);
    }
    return mutateRepoTree("create_folder", { name });
  }

  /** Delete a folder: the backend cascades to its PDFs (index + files). */
  function deleteFolder(name: string): Promise<boolean> {
    return mutateRepoTree("delete_folder", { name });
  }

  /** Delete a PDF (index + files); if it was open, go to the empty state. */
  async function deletePdf(pdf: PDFStruct): Promise<boolean> {
    const ok = await mutateRepoTree("delete_pdf", { id: pdf.id });
    if (ok) {
      delete pdfs.value[pdf.id];
      if (currentPdfId.value === pdf.id) currentPdfId.value = null;
    }
    return ok;
  }

  /** Move a PDF: belong=folder name (created if missing), null = repo root. */
  function movePdf(id: string, belong: string | null): Promise<boolean> {
    return mutateRepoTree("move_pdf", { id, belong });
  }

  /** Clear parse state: backend rebuilds an empty 1..=N skeleton (PDF kept), the frontend
   *  drops its cache, reloads if open, and wakes the scheduler.
   *  Page count: pdfjs value for the open book, 0 otherwise (backend reuses the JSON count). */
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
    if (currentPdfId.value === pdf.id) await selectPdf(pdf); // reload the new skeleton
    toast(t("library.clearDone", { name: pdf.name }), "info");
    useParseStore().wake(); // reset state = new work available
    return true;
  }

  /** Fetch the repo index; apply + persist the path on success, leave state on failure. */
  async function loadRepo(root: string): Promise<void> {
    const index = await invoke<RepoTree>("gettree_from_config", { root });
    repoRoot.value = root;
    repoIndex.value = index; // flat index stored as-is, no tree conversion
    void useSettingsStore().setRepoPath(root); // default for the next launch
    useParseStore().wake(); // repo switch/refresh = new work, try to continue the chain
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
