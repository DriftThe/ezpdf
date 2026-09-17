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
  /** Off: OCR text is stored as its own translation and marked done (no LLM, no null). Default on. */
  translateEnabled: boolean;
  /** pi-ai catalog provider id or "custom"; presets only supply endpoint + protocol/thinking compat. */
  provider: string;
  /** Compat snapshot: derived from the catalog for presets; for custom, only api is user-picked. */
  preset: LlmPresetCompat;
  baseUrl: string;
  apiKey: string; // plain text in config.json
  model: string;
  targetLang: string;
  /** Smart context: agent loop asks for neighbouring-page context on truncated text. Default on. */
  smartContext: boolean;
  /** Thinking-off strategy: "auto" | "reasoning" | "enable_thinking" | "thinking_type" | "none". */
  thinkingOff: string;
}

/** Flat LLM config sent to Rust (translate.rs LlmConfig, serde camelCase). */
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
  /** Block types to translate (Rust translateTypes; empty = backend default). */
  translateTypes: string[];
  /** Rust translateEnabled; false = source stored as its own translation. */
  translateEnabled: boolean;
}

/**
 * - autoLaunch: wake the OCR service on start (only when env + models are ready).
 * - resumeOnStart: resume unfinished parsing on start; both must be on to auto-run.
 * - theme: system/light/dark.
 * Both startup toggles default off (startup is paused).
 */
export type ThemeMode = "system" | "light" | "dark";

interface GeneralSettings {
  autoLaunch: boolean;
  resumeOnStart: boolean;
  theme: ThemeMode;
  /** UI locale; first launch derives it from the system language. */
  lang: AppLocale;
  /**
   * Checked block types (lib/blocks.ts BLOCK_TYPE_OPTIONS): sent to the LLM and cover-rendered.
   * Unchecked types keep original pixels; already-translated pages are not retried.
   */
  translateTypes: string[];
}

/**
 * local: app-managed pyserver (spawn + ready handshake + stdin-EOF exit; needs deps + models).
 * online: remote service over the same HTTP protocol (pyserver/PROTOCOL.md); needs only a URL.
 */
type OcrMode = "local" | "online";

/** OCR settings: install mirror + service source. */
interface OcrSettings {
  installMirror: boolean;
  mode: OcrMode;
  /** Remote parse URL (e.g. http://127.0.0.1:9055); ignored in local mode. */
  url: string;
  /** Remote token (token.txt); empty when the server has no auth. */
  token: string;
}

/**
 * Settings nav section. Adding one: extend this union, add a section component
 * (root class "set-pane"), then register it in SettingsPage.vue SECTIONS.
 * Extending the union without registering is a build error.
 */
export type SettingsSection = "llm" | "ocr" | "common";

/** Dev-only: parse LLM config from the gitignored root auth.cfg so it never enters a bundle.
 *  Missing or malformed file is silent. */
function parseAuthCfg(text: string): Partial<LlmSettings> {
  const out: Partial<LlmSettings> = {};
  for (const line of text.split(/\r?\n/)) {
    const m = /^\s*(baseUrl|apiKey|model|targetLang)\s*:\s*"([^"]*)"\s*,?\s*$/.exec(line);
    if (m) (out as Record<string, string>)[m[1]] = m[2];
  }
  return out;
}

async function fillLlmFromAuthCfg(llm: { value: LlmSettings }): Promise<void> {
  if (!import.meta.env.DEV) return; // dead-code eliminated in production builds
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
    /* missing/unreadable auth.cfg: keep empty config (translation skipped) */
  }
}

/** config.json shape (save_settings writes the whole file, load_settings reads it whole). */
interface PersistedConfig {
  llm?: Partial<LlmSettings>;
  general?: Partial<GeneralSettings>;
  ocr?: Partial<OcrSettings>;
  /** Last repo root (auto-opened on start; null = none). */
  repo?: string | null;
}

/** Shallow-merge section by section; unknown fields or type mismatches are ignored. */
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
  /** Whole settings page open (overlays sidebar + reader; toolbar stays; nothing unmounts). */
  const pageOpen = ref(false);
  /** Selected settings section. */
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

  /** pi-ai catalog (lazy; null = not loaded; browser-only mode never loads it). */
  const catalog = ref<PiProvider[] | null>(null);

  /** Lazy idempotent catalog load; failures are silent (manual Base URL still works). */
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

  /** Usable presets: providers with at least one model the client supports. */
  const usableProviders = computed<PiProvider[]>(() =>
    sortProviders((catalog.value ?? []).filter((p) => usableModels(p).length > 0)),
  );
  /** Presets whose protocol is unsupported (recognized only). */
  const otherProviders = computed<PiProvider[]>(() =>
    sortProviders((catalog.value ?? []).filter((p) => usableModels(p).length === 0)),
  );
  /** Current preset provider (custom = null). */
  const presetProvider = computed<PiProvider | null>(() =>
    findProvider(catalog.value ?? [], llm.value.provider),
  );

  /** Verify/model-fetch in flight (button re-entry guard + label). */
  const verifying = ref(false);
  const modelsFetching = ref(false);
  /** /models result (datalist for the model input). */
  const modelOptions = ref<string[]>([]);

  /** App version + update-check state (shown in the Common pane; silent check once at launch). */
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

  /** Last repo root persisted to config.json; library auto-opens it. */
  const repoPath = ref<string | null>(null);

  /** Load persisted config once (idempotent promise): auth.cfg first, then config overrides it.
   *  dev = repo root, prod = app dir. */
  let loadPromise: Promise<void> | null = null;
  /** Writes are gated on a successful load; otherwise default values would wipe the user's config. */
  let loaded = false;
  /** Locale as loaded from disk; doSave keeps it from being clobbered by defaults. */
  let loadedLang: AppLocale | null = null;
  /** User explicitly changed the locale; only then may it be written back. */
  let langTouched = false;
  function ensureLoaded(): Promise<void> {
    loadPromise ??= (async () => {
      if (!isTauri) return;
      try {
        appVersion.value = await getVersion();
      } catch {
        /* a missing version is not fatal (the update check fills it in later) */
      }
      try {
        const text = await invoke<string | null>("load_settings");
        // no config file (first launch): don't return yet — locale/target defaults still apply below
        if (text) {
          const cfg = JSON.parse(text) as PersistedConfig;
          // preset is nested: validate it separately instead of shallow-merging whole
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
        return; // load failed: keep loaded=false so this session never writes
      }
      loaded = true;
      // invalid config locale falls back to system detection; config wins over the localStorage mirror
      if (!isAppLocale(general.value.lang)) general.value.lang = detectLocale();
      setLocale(general.value.lang);
      loadedLang = general.value.lang;
      // target language default follows the system language; only filled when never set
      if (!llm.value.targetLang.trim()) llm.value.targetLang = defaultTargetLang(general.value.lang);
      // filter invalid/stale type tags; empty set = unset → built-in default (same in Rust)
      const picked = Array.isArray(general.value.translateTypes) ? general.value.translateTypes : [];
      const valid = picked.filter((t) => typeof t === "string" && BLOCK_TYPE_OPTIONS.includes(t));
      general.value.translateTypes = valid.length ? valid : [...DEFAULT_TRANSLATED_TYPES];
      // migrate old configs (baseUrl+model only): reverse-lookup provider, recompute the snapshot
      await ensureCatalog();
      if (
        llm.value.provider !== CUSTOM_PROVIDER &&
        !findProvider(catalog.value ?? [], llm.value.provider)
      ) {
        llm.value.provider = CUSTOM_PROVIDER; // provider vanished after a catalog upgrade
      }
      if (llm.value.provider === CUSTOM_PROVIDER && llm.value.baseUrl.trim()) {
        llm.value.provider = detectProviderId(catalog.value ?? [], llm.value.baseUrl);
      }
      syncPreset();
      autosaveArmed = true; // in-memory values now come from disk; autosave may start
    })();
    return loadPromise;
  }

  async function fillDefaults(): Promise<void> {
    await fillLlmFromAuthCfg(llm);
    await ensureLoaded();
  }
  void fillDefaults();

  /**
   * Recompute the compat snapshot from provider + model (empty when not in the catalog).
   * Custom keeps the user's picked protocol; otherwise editing the model name would reset it.
   */
  function syncPreset(): void {
    const preset = presetCompat(
      findModel(catalog.value ?? [], llm.value.provider, llm.value.model),
    );
    if (llm.value.provider === CUSTOM_PROVIDER) {
      preset.api = isSupportedApi(llm.value.preset.api) ? llm.value.preset.api : DEFAULT_API;
    }
    llm.value.preset = preset;
  }

  /** Recompute the snapshot after typing a model (input change). */
  function applyModelInput(): void {
    syncPreset();
  }

  /** Effective protocol: from the catalog for presets, user-picked for custom; empty = Chat. */
  const protocol = computed<string>(() =>
    isSupportedApi(llm.value.preset.api) ? llm.value.preset.api : DEFAULT_API,
  );

  /** Pick protocol: custom endpoints only; presets get theirs from the catalog. */
  function applyProtocol(api: string): void {
    if (llm.value.provider !== CUSTOM_PROVIDER || !isSupportedApi(api)) return;
    llm.value.preset = { ...llm.value.preset, api };
  }

  /** Select a provider: fill the endpoint; switch to its first usable model if needed. */
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

  /** Flat config for invoke (scheduler + verify_llm); null when baseUrl/apiKey/model are missing. */
  function llmInvokePayload(): LlmInvokePayload | null {
    const s = llm.value;
    // send even with translation off: Rust copies source into translation and marks the
    // page done (no network), so endpoint/key may be empty and a bare install still finishes OCR text
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
    // a collapsed sidebar makes the toolbar full-width and cover the back button; expand first
    useLibraryStore().sidebarOpen = true;
  }

  /** Debounced autosave: any change writes config.json 2 s later (whole-file write).
   *  Skipped outside Tauri and before a successful load. */
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
          console.warn("[settings] 自动保存失败:", e); // quiet: the next change or exit writes again
        }
      })();
    }, AUTOSAVE_DEBOUNCE_MS);
  }

  // sync flush: assignments during load (auth.cfg, defaults, preset migration) must not write
  watch([llm, general, ocr, repoPath], scheduleAutosave, { deep: true, flush: "sync" });

  /** Whole-file write (autosave / back button / immediate persistence). */
  async function doSave(): Promise<void> {
    cancelAutosave(); // this write supersedes a pending autosave
    // locale may only change via setLang: a drift from the loaded value (with no user
    // action) means some default flow clobbered it, so revert before writing.
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

  /** Save and close (back button): errors stay on the page; success is silent. */
  async function save(): Promise<void> {
    if (!isTauri) {
      pageOpen.value = false;
      return;
    }
    if (!loaded) {
      // load never succeeded: writing would wipe the config, so stay on the page
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

  /** Immediate persistence for instant-effect settings: no toast, console warn on failure. */
  async function persistNow(what: string): Promise<void> {
    if (!isTauri) return;
    try {
      await doSave();
    } catch (e) {
      console.warn(`[settings] ${what}持久化失败:`, e);
    }
  }

  /** Persist repo choice immediately (failure doesn't block opening the repo). */
  async function setRepoPath(path: string | null): Promise<void> {
    repoPath.value = path;
    await persistNow("仓库路径");
  }

  /** Persist theme immediately; theme.ts maintains the localStorage mirror. */
  async function setTheme(mode: ThemeMode): Promise<void> {
    general.value.theme = mode;
    await persistNow("主题");
  }

  /** Switch locale: immediate effect + persistence; i18n.ts keeps the localStorage mirror. */
  async function setLang(locale: AppLocale): Promise<void> {
    general.value.lang = locale;
    langTouched = true;
    setLocale(locale);
    await persistNow("界面语言");
  }

  /** Verify LLM: connectivity + thinking-off probe; writes the strategy back to thinkingOff. */
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
      // warn only if thinking content came back; "none" may just mean no params needed.
      // no warning for presets marked as non-reasoning.
      const warn = report.thinkingOn && !report.presetNoThinking;
      toast(report.message, warn ? "warn" : "info");
    } catch (e) {
      toast(t("llm.verifyFailed", { error: String(e) }), "error");
    } finally {
      verifying.value = false;
    }
  }

  /** Fetch /models: toast on failure, silent on success. */
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

  /** Check GitHub Releases vs current; manual=false (launch) is silent, manual=true toasts. */
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
