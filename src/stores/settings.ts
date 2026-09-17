import { defineStore } from "pinia";
import { computed, ref, watch } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { getVersion } from "@tauri-apps/api/app";
import type { LlmVerifyReport } from "../../src-tauri/bindings/LlmVerifyReport";
import type { UpdateInfo } from "../../src-tauri/bindings/UpdateInfo";
import type { PiProvider } from "../lib/piModels.generated";
import {
  CUSTOM_PROVIDER,
  DEFAULT_API,
  EMPTY_PRESET,
  type LlmPresetCompat,
  detectProviderId,
  findModel,
  findProvider,
  isSupportedApi,
  loadCatalog,
  presetCompat,
  sortProviders,
  usableModels,
} from "../lib/piModels";
import { toast } from "../composables/toast";
import { isTauri } from "../lib/env";
import { useLibraryStore } from "./library";
import { BLOCK_TYPE_OPTIONS, DEFAULT_TRANSLATED_TYPES } from "../lib/blocks";
import {
  currentLocale,
  defaultTargetLang,
  detectLocale,
  isAppLocale,
  setLocale,
  t,
} from "../lib/i18n";
import type { AppLocale } from "../locales";

/** Non-Tauri (browser pnpm dev): invoke always fails, so persistence is silent. */

interface LlmSettings {
  /** Off = store OCR text as its own translation, no LLM (bypass). Default on. */
  translateEnabled: boolean;
  /** pi-ai provider id or "custom"; presets supply endpoint + protocol/thinking compat. */
  provider: string;
  /** Compat snapshot from the catalog; for custom only api is user-picked. */
  preset: LlmPresetCompat;
  baseUrl: string;
  apiKey: string; // plain text in config.json
  model: string;
  targetLang: string;
  /** Agent loop may ask for neighbouring-page context on truncated text. Default on. */
  smartContext: boolean;
  /** Thinking-off strategy; "auto" probes. */
  thinkingOff: string;
}

export interface LlmInvokePayload {
  baseUrl: string;
  apiKey: string;
  model: string;
  targetLang: string;
  smartContext: boolean;
  thinkingOff: string;
  api: string;
  thinkingOffKind: string;
  thinkingOffValue: string | null;
  maxTokensField: string;
  modelReasoning: boolean | null;
  extraHeaders: Record<string, string>;
  translateTypes: string[];
  translateEnabled: boolean;
}

export type ThemeMode = "system" | "light" | "dark";

interface GeneralSettings {
  autoLaunch: boolean;
  resumeOnStart: boolean;
  theme: ThemeMode;
  lang: AppLocale;
  /** Checked types go to the LLM/cover; a change only affects untranslated pages. */
  translateTypes: string[];
}

/** local needs deps + models; online is a remote URL over the same protocol. */
type OcrMode = "local" | "online";

interface OcrSettings {
  installMirror: boolean;
  mode: OcrMode;
  url: string;
  token: string;
}

/** Registering a new section in SettingsPage.vue SECTIONS is mandatory (build error otherwise). */
export type SettingsSection = "llm" | "ocr" | "common";

/** Dev-only: read the gitignored auth.cfg (never bundled); missing/malformed is silent. */
function parseAuthCfg(text: string): Partial<LlmSettings> {
  const out: Partial<LlmSettings> = {};
  for (const line of text.split(/\r?\n/)) {
    const m = /^\s*(baseUrl|apiKey|model|targetLang)\s*:\s*"([^"]*)"\s*,?\s*$/.exec(line);
    if (m) (out as Record<string, string>)[m[1]] = m[2];
  }
  return out;
}

async function fillLlmFromAuthCfg(llm: { value: LlmSettings }): Promise<void> {
  if (!import.meta.env.DEV) return; // dead-code eliminated in production
  const files = import.meta.glob("../../auth.cfg", { query: "?raw", import: "default" }) as Record<
    string,
    () => Promise<string>
  >;
  const loader = files["../../auth.cfg"];
  if (!loader) return;
  try {
    const parsed = parseAuthCfg(await loader());
    llm.value = { ...llm.value, ...parsed };
  } catch {
  }
}

interface PersistedConfig {
  llm?: Partial<LlmSettings>;
  general?: Partial<GeneralSettings>;
  ocr?: Partial<OcrSettings>;
  repo?: string | null;
}

function isRecord(v: unknown): v is Record<string, unknown> {
  return !!v && typeof v === "object" && !Array.isArray(v);
}
function mergeSection<T extends object>(target: T, patch: unknown): void {
  if (!isRecord(patch)) return;
  for (const key of Object.keys(target) as Array<keyof T>) {
    const v = patch[key as string];
    if (typeof v === typeof target[key]) target[key] = v as T[keyof T];
  }
}

/** preset is nested (shallow merge would replace it whole), so validate field by field. */
function mergePreset(target: LlmPresetCompat, patch: unknown): void {
  if (!isRecord(patch)) return;
  for (const key of Object.keys(target) as Array<keyof LlmPresetCompat>) {
    const v = patch[key as string];
    if (key === "extraHeaders") {
      if (isRecord(v)) {
        const headers: Record<string, string> = {};
        for (const [hk, hv] of Object.entries(v)) if (typeof hv === "string") headers[hk] = hv;
        target.extraHeaders = headers;
      }
      continue;
    }
    // nullable fields default to null; typeof null === "object" so compare against the real type
    if (key === "thinkingOffValue") {
      if (v === null || typeof v === "string") target.thinkingOffValue = v;
      continue;
    }
    if (key === "reasoning") {
      if (v === null || typeof v === "boolean") target.reasoning = v;
      continue;
    }
    if (typeof v === typeof target[key]) target[key] = v as never;
  }
}

export const useSettingsStore = defineStore("settings", () => {
  const pageOpen = ref(false);
  const section = ref<SettingsSection>("llm");

  const llm = ref<LlmSettings>({
    provider: CUSTOM_PROVIDER,
    preset: { ...EMPTY_PRESET, extraHeaders: {} },
    baseUrl: "",
    apiKey: "",
    model: "",
    targetLang: "",
    smartContext: true,
    thinkingOff: "auto",
    translateEnabled: true,
  });

  const catalog = ref<PiProvider[] | null>(null);

  let catalogPromise: Promise<PiProvider[] | null> | null = null;
  function ensureCatalog(): Promise<PiProvider[] | null> {
    catalogPromise ??= loadCatalog()
      .then((list) => {
        catalog.value = list;
        return list;
      })
      .catch((e) => {
        console.warn("[settings] pi-ai 目录加载失败:", e);
        return null;
      });
    return catalogPromise;
  }

  const usableProviders = computed<PiProvider[]>(() =>
    sortProviders((catalog.value ?? []).filter((p) => usableModels(p).length > 0)),
  );
  const otherProviders = computed<PiProvider[]>(() =>
    sortProviders((catalog.value ?? []).filter((p) => usableModels(p).length === 0)),
  );
  const presetProvider = computed<PiProvider | null>(() =>
    findProvider(catalog.value ?? [], llm.value.provider),
  );

  const verifying = ref(false);
  const modelsFetching = ref(false);
  const modelOptions = ref<string[]>([]);

  const appVersion = ref("");
  const updateBusy = ref(false);
  const updateText = ref(t("update.notChecked"));

  const general = ref<GeneralSettings>({
    autoLaunch: false,
    resumeOnStart: false,
    theme: "system",
    lang: currentLocale(),
    translateTypes: [...DEFAULT_TRANSLATED_TYPES],
  });

  const ocr = ref<OcrSettings>({
    installMirror: true,
    mode: "local",
    url: "",
    token: "",
  });

  const repoPath = ref<string | null>(null);

  let loadPromise: Promise<void> | null = null;
  /** Writes are gated on a successful load; otherwise defaults would wipe the user's config. */
  let loaded = false;
  /** Locale as loaded from disk; doSave reverts a default-flow drift unless the user changed it. */
  let loadedLang: AppLocale | null = null;
  /** Set by setLang; only then may the locale be written back. */
  let langTouched = false;
  function ensureLoaded(): Promise<void> {
    loadPromise ??= (async () => {
      if (!isTauri) return;
      try {
        appVersion.value = await getVersion();
      } catch {
      }
      try {
        const text = await invoke<string | null>("load_settings");
        // no config file (first launch): don't return yet — locale/target defaults still apply below
        if (text) {
          const cfg = JSON.parse(text) as PersistedConfig;
          const patch: Record<string, unknown> = isRecord(cfg.llm) ? { ...cfg.llm } : {};
          delete patch.preset;
          mergeSection(llm.value, patch);
          mergePreset(llm.value.preset, cfg.llm?.preset);
          mergeSection(general.value, cfg.general);
          mergeSection(ocr.value, cfg.ocr);
          if (typeof cfg.repo === "string" && cfg.repo.trim()) repoPath.value = cfg.repo;
        }
      } catch (e) {
        console.warn("[settings] config.json 读取失败，使用默认值:", e);
        return; // load failed: keep loaded=false, never write this session
      }
      loaded = true;
      // config wins over the localStorage mirror; invalid falls back to detection
      if (!isAppLocale(general.value.lang)) general.value.lang = detectLocale();
      setLocale(general.value.lang);
      loadedLang = general.value.lang;
      if (!llm.value.targetLang.trim()) llm.value.targetLang = defaultTargetLang(general.value.lang);
      const picked = Array.isArray(general.value.translateTypes) ? general.value.translateTypes : [];
      const valid = picked.filter((t) => typeof t === "string" && BLOCK_TYPE_OPTIONS.includes(t));
      general.value.translateTypes = valid.length ? valid : [...DEFAULT_TRANSLATED_TYPES];
      await ensureCatalog();
      if (
        llm.value.provider !== CUSTOM_PROVIDER &&
        !findProvider(catalog.value ?? [], llm.value.provider)
      ) {
        llm.value.provider = CUSTOM_PROVIDER;
      }
      if (llm.value.provider === CUSTOM_PROVIDER && llm.value.baseUrl.trim()) {
        llm.value.provider = detectProviderId(catalog.value ?? [], llm.value.baseUrl);
      }
      syncPreset();
      autosaveArmed = true; // values now come from disk; autosave may start
    })();
    return loadPromise;
  }

  async function fillDefaults(): Promise<void> {
    await fillLlmFromAuthCfg(llm);
    await ensureLoaded();
  }
  void fillDefaults();

  /** Recompute the snapshot; custom keeps its picked protocol (editing the model would reset it). */
  function syncPreset(): void {
    const preset = presetCompat(
      findModel(catalog.value ?? [], llm.value.provider, llm.value.model),
    );
    if (llm.value.provider === CUSTOM_PROVIDER) {
      preset.api = isSupportedApi(llm.value.preset.api) ? llm.value.preset.api : DEFAULT_API;
    }
    llm.value.preset = preset;
  }

  function applyModelInput(): void {
    syncPreset();
  }

  const protocol = computed<string>(() =>
    isSupportedApi(llm.value.preset.api) ? llm.value.preset.api : DEFAULT_API,
  );

  function applyProtocol(api: string): void {
    if (llm.value.provider !== CUSTOM_PROVIDER || !isSupportedApi(api)) return;
    llm.value.preset = { ...llm.value.preset, api };
  }

  function applyProvider(id: string): void {
    llm.value.provider = id;
    if (id === CUSTOM_PROVIDER) {
      syncPreset();
      return;
    }
    const provider = findProvider(catalog.value ?? [], id);
    if (!provider) return;
    if (!findModel(catalog.value ?? [], id, llm.value.model)) {
      const first = usableModels(provider)[0] ?? provider.models[0];
      if (first) llm.value.model = first.id;
    }
    const model = findModel(catalog.value ?? [], id, llm.value.model) ?? provider.models[0];
    if (model) llm.value.baseUrl = model.baseUrl;
    syncPreset();
  }

  function llmInvokePayload(): LlmInvokePayload | null {
    const s = llm.value;
    // send even with translation off: Rust's bypass copies source text (no network, no keys)
    if (s.translateEnabled && (!s.baseUrl.trim() || !s.apiKey.trim() || !s.model.trim())) {
      return null;
    }
    return {
      baseUrl: s.baseUrl.trim(),
      apiKey: s.apiKey,
      model: s.model.trim(),
      targetLang: s.targetLang,
      smartContext: s.smartContext,
      thinkingOff: s.thinkingOff,
      api: s.preset.api,
      thinkingOffKind: s.preset.thinkingOffKind,
      thinkingOffValue: s.preset.thinkingOffValue,
      maxTokensField: s.preset.maxTokensField,
      modelReasoning: s.preset.reasoning,
      extraHeaders: s.preset.extraHeaders,
      translateTypes: [...general.value.translateTypes],
      translateEnabled: s.translateEnabled,
    };
  }

  function openPage(): void {
    pageOpen.value = true;
    // a collapsed sidebar would let the toolbar cover the back button
    useLibraryStore().sidebarOpen = true;
  }

  /** Autosave 2 s after the last change (whole file); skipped outside Tauri and before a load. */
  const AUTOSAVE_DEBOUNCE_MS = 2000;
  let autosaveTimer: ReturnType<typeof setTimeout> | null = null;
  let autosaveArmed = false;

  function cancelAutosave(): void {
    if (autosaveTimer !== null) {
      clearTimeout(autosaveTimer);
      autosaveTimer = null;
    }
  }

  function scheduleAutosave(): void {
    if (!isTauri || !autosaveArmed) return;
    cancelAutosave();
    autosaveTimer = setTimeout(() => {
      autosaveTimer = null;
      void (async () => {
        try {
          await doSave();
        } catch (e) {
          console.warn("[settings] 自动保存失败:", e); // quiet: the next change writes again
        }
      })();
    }, AUTOSAVE_DEBOUNCE_MS);
  }

  // sync flush: assignments during load must not trigger a write
  watch([llm, general, ocr, repoPath], scheduleAutosave, { deep: true, flush: "sync" });

  async function doSave(): Promise<void> {
    cancelAutosave(); // this write supersedes a pending autosave
    // locale may only change via setLang; revert a default-flow drift before writing
    if (!langTouched && loadedLang !== null && general.value.lang !== loadedLang) {
      console.warn(`[settings] lang reverted to ${general.value.lang}, keeping ${loadedLang}`);
      general.value.lang = loadedLang;
    }
    if (!loaded) {
      console.error("[settings] save skipped: the config was never loaded successfully");
      return;
    }
    const payload: PersistedConfig = {
      llm: { ...llm.value },
      general: { ...general.value },
      ocr: { ...ocr.value },
      repo: repoPath.value,
    };
    await invoke("save_settings", { json: JSON.stringify(payload, null, 2) });
  }

  async function save(): Promise<void> {
    if (!isTauri) {
      pageOpen.value = false;
      return;
    }
    if (!loaded) {
      // writing would wipe the config; stay on the page
      toast(t("settings.saveNotLoaded"), "error");
      return;
    }
    try {
      await doSave();
      pageOpen.value = false;
    } catch (e) {
      toast(t("settings.saveFailed", { error: String(e) }), "error");
    }
  }

  async function persistNow(what: string): Promise<void> {
    if (!isTauri) return;
    try {
      await doSave();
    } catch (e) {
      console.warn(`[settings] ${what}持久化失败:`, e);
    }
  }

  async function setRepoPath(path: string | null): Promise<void> {
    repoPath.value = path;
    await persistNow("仓库路径");
  }

  async function setTheme(mode: ThemeMode): Promise<void> {
    general.value.theme = mode;
    await persistNow("主题");
  }

  /** Locale takes effect immediately and persists; i18n.ts keeps the localStorage mirror. */
  async function setLang(locale: AppLocale): Promise<void> {
    general.value.lang = locale;
    langTouched = true;
    setLocale(locale);
    await persistNow("界面语言");
  }

  /** Connectivity + thinking-off probe; writes the chosen strategy back to thinkingOff. */
  async function verifyLlm(): Promise<void> {
    if (verifying.value) return;
    const payload = llmInvokePayload();
    if (!payload) {
      toast(t("llm.fillAllRequired"), "warn");
      return;
    }
    verifying.value = true;
    try {
      const report = await invoke<LlmVerifyReport>("verify_llm", { llm: payload });
      llm.value.thinkingOff = report.strategy;
      // warn only on real thinking content; non-reasoning presets are exempt
      const warn = report.thinkingOn && !report.presetNoThinking;
      toast(report.message, warn ? "warn" : "info");
    } catch (e) {
      toast(t("llm.verifyFailed", { error: String(e) }), "error");
    } finally {
      verifying.value = false;
    }
  }

  async function fetchModels(): Promise<void> {
    if (modelsFetching.value) return;
    const { baseUrl, apiKey } = llm.value;
    if (!baseUrl.trim() || !apiKey.trim()) {
      toast(t("llm.fillBaseKey"), "warn");
      return;
    }
    modelsFetching.value = true;
    try {
      modelOptions.value = await invoke<string[]>("fetch_llm_models", {
        baseUrl,
        apiKey,
        api: protocol.value,
      });
      toast(t("llm.modelsFetched", { count: modelOptions.value.length }));
    } catch (e) {
      toast(t("llm.fetchModelsFailed", { error: String(e) }), "error");
    } finally {
      modelsFetching.value = false;
    }
  }

  /** manual=false (launch) is silent; manual=true toasts errors. */
  async function checkUpdate(manual = false): Promise<void> {
    if (!isTauri || updateBusy.value) return;
    updateBusy.value = true;
    try {
      const info = await invoke<UpdateInfo>("check_update");
      appVersion.value = info.current;
      if (info.newer && info.latest) {
        updateText.value = t("update.found", { latest: info.latest });
        toast(t("update.foundToast", { latest: info.latest, current: info.current }), "info");
      } else {
        updateText.value = t("update.upToDate", { current: info.current });
      }
    } catch (e) {
      updateText.value = t("update.failed");
      if (manual) toast(t("update.failedToast", { error: String(e) }), "warn");
    } finally {
      updateBusy.value = false;
    }
  }

  return {
    pageOpen,
    section,
    llm,
    general,
    ocr,
    repoPath,
    verifying,
    modelsFetching,
    modelOptions,
    appVersion,
    updateBusy,
    updateText,
    usableProviders,
    otherProviders,
    presetProvider,
    protocol,
    openPage,
    save,
    setRepoPath,
    setTheme,
    setLang,
    ensureCatalog,
    applyProvider,
    applyProtocol,
    applyModelInput,
    llmInvokePayload,
    verifyLlm,
    fetchModels,
    checkUpdate,
    ensureLoaded,
  };

});
