// pi-ai 目录（scripts/sync-pi-models.mjs 生成）的封装：供应商显示名、分组、兼容性快照。
// 定位：LLM 设置页的「供应商 / 模型预设」数据源 + 给 Rust 客户端的协议适配快照。
// 运行时网络请求仍在 Rust（translate.rs 的三种协议），这里只提供目录与元数据。
import type { PiModel, PiProvider } from "./piModels.generated";
import { currentLocale } from "./i18n";

/** Rust 客户端实现的三种线协议（pi-ai 目录的 api 字段；见 docs/protocols.md） */
export const SUPPORTED_APIS = ["openai-completions", "anthropic-messages", "openai-responses"] as const;

/** 协议 id（也是 pi-ai api 字段的取值） */
export type SupportedApi = (typeof SUPPORTED_APIS)[number];

/** 自定义端点的默认协议（旧配置 `api` 为空也按它处理） */
export const DEFAULT_API: SupportedApi = "openai-completions";

/**
 * 协议显示名：三种协议名各语言通用，所以是数据不是 i18n key（与 PROVIDER_LABELS 同理，
 * 说明文字才进目录）。顺序即下拉顺序。
 */
export const PROTOCOL_LABELS: Record<SupportedApi, string> = {
  "openai-completions": "Chat Completions",
  "anthropic-messages": "Messages",
  "openai-responses": "Responses",
};

export function isSupportedApi(api: string): api is SupportedApi {
  return (SUPPORTED_APIS as readonly string[]).includes(api);
}

/** 自定义供应商（不走目录：手填 Base URL + 模型，关思考靠验证按钮探测） */
export const CUSTOM_PROVIDER = "custom";

/**
 * 供应商显示名（pi-ai 目录只有 id）。用户 2026-09-15 要求「方便识别」，所以这里用人话 + 品牌：
 * 冷门供应商直接沿用 id。显示名是数据不是 i18n key（见 AGENTS），中英同一张表：
 * 一个供应商一行，避免两张表键集悄悄漂移。
 */
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

/** 排序权重：常用/可直连的国内可达服务排前面（未列出的按显示名排序） */
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

/** 目录懒加载：单独 chunk（~400KB），只在设置页/迁移需要时拉取 */
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

/** 旧配置迁移/Base URL 手填：按端点反查预设供应商（容忍结尾斜杠与大小写） */
export function detectProviderId(catalog: PiProvider[], baseUrl: string): string {
  const url = baseUrl.trim().replace(/\/+$/, "").toLowerCase();
  if (!url) return CUSTOM_PROVIDER;
  for (const p of catalog) {
    // 逐模型比对（同一供应商可能有多个端点，如 opencode 的 /zen 与 /zen/v1）
    if (p.models.some((m) => m.baseUrl.replace(/\/+$/, "").toLowerCase() === url)) return p.id;
  }
  for (const p of catalog) {
    const base = p.baseUrl.replace(/\/+$/, "").toLowerCase();
    if (base && (url.startsWith(base) || base.startsWith(url))) return p.id;
  }
  return CUSTOM_PROVIDER;
}

/**
 * 兼容性快照（随设置持久化）：选预设模型时算一次，冷启动直接可用（不必等目录加载），
 * 由 Rust 的 chat_request 施加。自定义端点全空 = 退回旧行为（URL 规则 + 验证探测）。
 */
export interface LlmPresetCompat {
  /** pi-ai 协议；"" = 未知（自定义端点按 openai-completions 处理） */
  api: string;
  /** 施加关思考参数的方式（none = 预设表示无法通过参数关闭） */
  thinkingOffKind: "none" | "thinking_type" | "enable_thinking" | "chat_template_kwargs" | "reasoning_effort" | "";
  thinkingOffValue: string | null;
  /** "" = 保持 max_tokens；"max_completion_tokens" = 按 pi-ai compat 换字段名 */
  maxTokensField: string;
  /** 预设模型的 reasoning 标记；null = 未知 */
  reasoning: boolean | null;
  /** 模型特有请求头（pi-ai 目录里少数模型有） */
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
