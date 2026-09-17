// Wrapper over the pi-ai catalog (generated): provider labels, grouping, compat snapshots.
import type { PiModel, PiProvider } from "./piModels.generated";
import { currentLocale } from "./i18n";

export const SUPPORTED_APIS = ["openai-completions", "anthropic-messages", "openai-responses"] as const;

export type SupportedApi = (typeof SUPPORTED_APIS)[number];

/** Default for custom endpoints; an empty legacy api also means this. */
export const DEFAULT_API: SupportedApi = "openai-completions";

export const PROTOCOL_LABELS: Record<SupportedApi, string> = {
  "openai-completions": "Chat Completions",
  "anthropic-messages": "Messages",
  "openai-responses": "Responses",
};

export function isSupportedApi(api: string): api is SupportedApi {
  return (SUPPORTED_APIS as readonly string[]).includes(api);
}

export const CUSTOM_PROVIDER = "custom";

/** Display names are data, not i18n keys; unlisted providers use their id. */
const PROVIDER_LABELS: Record<string, { zh: string; en: string }> = {
  "opencode-go": { zh: "OpenCode Zen（Go 套餐）", en: "OpenCode Zen (Go plan)" },
  opencode: { zh: "OpenCode Zen", en: "OpenCode Zen" },
  deepseek: { zh: "DeepSeek 深度求索", en: "DeepSeek" },
  zai: { zh: "Z.ai 智谱 GLM", en: "Z.ai GLM" },
  "moonshotai-cn": { zh: "Moonshot 月之暗面（国内）", en: "Moonshot (China)" },
  moonshotai: { zh: "Moonshot 月之暗面", en: "Moonshot" },
  xiaomi: { zh: "Xiaomi MiMo 小米", en: "Xiaomi MiMo" },
  "xiaomi-token-plan-cn": { zh: "Xiaomi MiMo（国内 Token 包）", en: "Xiaomi MiMo (China token plan)" },
  "xiaomi-token-plan-sgp": { zh: "Xiaomi MiMo（新加坡 Token 包）", en: "Xiaomi MiMo (Singapore token plan)" },
  "xiaomi-token-plan-ams": { zh: "Xiaomi MiMo（阿姆斯特丹 Token 包）", en: "Xiaomi MiMo (Amsterdam token plan)" },
  "minimax-cn": { zh: "MiniMax 稀宇（国内）", en: "MiniMax (China)" },
  minimax: { zh: "MiniMax 稀宇", en: "MiniMax" },
  "kimi-coding": { zh: "Kimi Coding", en: "Kimi Coding" },
  openrouter: { zh: "OpenRouter（聚合）", en: "OpenRouter (aggregator)" },
  huggingface: { zh: "Hugging Face（聚合）", en: "Hugging Face (aggregator)" },
  cerebras: { zh: "Cerebras", en: "Cerebras" },
  groq: { zh: "Groq", en: "Groq" },
  xai: { zh: "xAI Grok", en: "xAI Grok" },
  mistral: { zh: "Mistral", en: "Mistral" },
  fireworks: { zh: "Fireworks", en: "Fireworks" },
  "vercel-ai-gateway": { zh: "Vercel AI Gateway（聚合）", en: "Vercel AI Gateway (aggregator)" },
  "cloudflare-workers-ai": { zh: "Cloudflare Workers AI", en: "Cloudflare Workers AI" },
  "cloudflare-ai-gateway": { zh: "Cloudflare AI Gateway（聚合）", en: "Cloudflare AI Gateway (aggregator)" },
  openai: { zh: "OpenAI", en: "OpenAI" },
  "openai-codex": { zh: "OpenAI Codex", en: "OpenAI Codex" },
  anthropic: { zh: "Anthropic Claude", en: "Anthropic Claude" },
  google: { zh: "Google Gemini", en: "Google Gemini" },
  "google-vertex": { zh: "Google Vertex AI", en: "Google Vertex AI" },
  "azure-openai-responses": { zh: "Azure OpenAI", en: "Azure OpenAI" },
  "amazon-bedrock": { zh: "Amazon Bedrock", en: "Amazon Bedrock" },
  "github-copilot": { zh: "GitHub Copilot", en: "GitHub Copilot" },
};

const PROVIDER_PRIORITY = [
  "opencode-go",
  "opencode",
  "deepseek",
  "zai",
  "moonshotai-cn",
  "moonshotai",
  "xiaomi",
  "minimax-cn",
  "minimax",
  "openrouter",
  "huggingface",
  "groq",
  "cerebras",
  "xai",
  "kimi-coding",
];

export function providerLabel(id: string): string {
  const label = PROVIDER_LABELS[id];
  if (!label) return id;
  return currentLocale() === "en" ? label.en : label.zh;
}

export function sortProviders(providers: PiProvider[]): PiProvider[] {
  const rank = (p: PiProvider): [number, string] => {
    const i = PROVIDER_PRIORITY.indexOf(p.id);
    return [i === -1 ? PROVIDER_PRIORITY.length : i, providerLabel(p.id)];
  };
  return [...providers].sort((a, b) => {
    const [ra, la] = rank(a);
    const [rb, lb] = rank(b);
    return ra - rb || la.localeCompare(lb);
  });
}

function isUsable(model: PiModel): boolean {
  return isSupportedApi(model.api);
}

export function usableModels(provider: PiProvider): PiModel[] {
  return provider.models.filter(isUsable);
}

/** Lazy catalog load: a separate ~400 KB chunk, fetched only when needed. */
let loading: Promise<PiProvider[]> | null = null;
export function loadCatalog(): Promise<PiProvider[]> {
  loading ??= import("./piModels.generated").then((m) => m.PI_CATALOG);
  return loading;
}

export function findProvider(catalog: PiProvider[], id: string): PiProvider | null {
  return catalog.find((p) => p.id === id) ?? null;
}

export function findModel(catalog: PiProvider[], providerId: string, modelId: string): PiModel | null {
  const provider = findProvider(catalog, providerId);
  if (!provider) return null;
  return provider.models.find((m) => m.id === modelId) ?? null;
}

export function detectProviderId(catalog: PiProvider[], baseUrl: string): string {
  const url = baseUrl.trim().replace(/\/+$/, "").toLowerCase();
  if (!url) return CUSTOM_PROVIDER;
  for (const p of catalog) {
    // per model: a provider may have several endpoints
    if (p.models.some((m) => m.baseUrl.replace(/\/+$/, "").toLowerCase() === url)) return p.id;
  }
  for (const p of catalog) {
    const base = p.baseUrl.replace(/\/+$/, "").toLowerCase();
    if (base && (url.startsWith(base) || base.startsWith(url))) return p.id;
  }
  return CUSTOM_PROVIDER;
}

/** Computed on model pick so a cold start needs no catalog load; applied by Rust's chat_request. */
export interface LlmPresetCompat {
  api: string;
  thinkingOffKind: "none" | "thinking_type" | "enable_thinking" | "chat_template_kwargs" | "reasoning_effort" | "";
  thinkingOffValue: string | null;
  maxTokensField: string;
  reasoning: boolean | null;
  extraHeaders: Record<string, string>;
}

export const EMPTY_PRESET: LlmPresetCompat = {
  api: "",
  thinkingOffKind: "",
  thinkingOffValue: null,
  maxTokensField: "",
  reasoning: null,
  extraHeaders: {},
};

export function presetCompat(model: PiModel | null): LlmPresetCompat {
  if (!model) return { ...EMPTY_PRESET, extraHeaders: {} };
  return {
    api: model.api,
    thinkingOffKind: model.thinkingOffKind,
    thinkingOffValue: model.thinkingOffValue,
    maxTokensField: model.maxTokensField,
    reasoning: model.reasoning,
    extraHeaders: model.headers ?? {},
  };
}
