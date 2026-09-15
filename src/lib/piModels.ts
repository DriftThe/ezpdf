// pi-ai 目录（scripts/sync-pi-models.mjs 生成）的封装：供应商显示名、分组、兼容性快照。
// 定位：LLM 设置页的「供应商 / 模型预设」数据源 + 给 Rust 客户端的协议适配快照。
// 运行时网络请求仍在 Rust（translate.rs，OpenAI 兼容），这里只提供目录与元数据。
import type { PiModel, PiProvider } from "./piModels.generated";

/** 当前 Rust 客户端唯一支持的协议（其他协议只在 UI 里「可识别」，不可用） */
export const SUPPORTED_API = "openai-completions";

/** 自定义供应商（不走目录：手填 Base URL + 模型，关思考靠验证按钮探测） */
export const CUSTOM_PROVIDER = "custom";

/**
 * 供应商显示名（pi-ai 目录只有 id）。用户 2026-09-15 要求「方便识别」，
 * 所以这里用人话 + 品牌：冷门供应商直接沿用 id。
 */
export const PROVIDER_LABELS: Record<string, string> = {
  "opencode-go": "OpenCode Zen（Go 套餐）",
  opencode: "OpenCode Zen",
  deepseek: "DeepSeek 深度求索",
  zai: "Z.ai 智谱 GLM",
  "moonshotai-cn": "Moonshot 月之暗面（国内）",
  moonshotai: "Moonshot 月之暗面",
  xiaomi: "Xiaomi MiMo 小米",
  "xiaomi-token-plan-cn": "Xiaomi MiMo（国内 Token 包）",
  "xiaomi-token-plan-sgp": "Xiaomi MiMo（新加坡 Token 包）",
  "xiaomi-token-plan-ams": "Xiaomi MiMo（阿姆斯特丹 Token 包）",
  "minimax-cn": "MiniMax 稀宇（国内）",
  minimax: "MiniMax 稀宇",
  "kimi-coding": "Kimi Coding",
  openrouter: "OpenRouter（聚合）",
  huggingface: "Hugging Face（聚合）",
  cerebras: "Cerebras",
  groq: "Groq",
  xai: "xAI Grok",
  mistral: "Mistral",
  fireworks: "Fireworks",
  "vercel-ai-gateway": "Vercel AI Gateway（聚合）",
  "cloudflare-workers-ai": "Cloudflare Workers AI",
  "cloudflare-ai-gateway": "Cloudflare AI Gateway（聚合）",
  openai: "OpenAI",
  "openai-codex": "OpenAI Codex",
  anthropic: "Anthropic Claude",
  google: "Google Gemini",
  "google-vertex": "Google Vertex AI",
  "azure-openai-responses": "Azure OpenAI",
  "amazon-bedrock": "Amazon Bedrock",
  "github-copilot": "GitHub Copilot",
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
  return PROVIDER_LABELS[id] ?? id;
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

export function isUsable(model: PiModel): boolean {
  return model.api === SUPPORTED_API;
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
  /** pi-ai thinkingFormat；"" = 未知 */
  thinkingFormat: string;
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
  thinkingFormat: "",
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
    thinkingFormat: model.thinkingFormat,
    thinkingOffKind: model.thinkingOffKind,
    thinkingOffValue: model.thinkingOffValue,
    maxTokensField: model.maxTokensField,
    reasoning: model.reasoning,
    extraHeaders: model.headers ?? {},
  };
}

/** 关思考说明文案（设置页；预设已定则不再需要验证按钮探测） */
export function thinkingHint(compat: LlmPresetCompat, strategy: string): string {
  if (compat.api && compat.api !== SUPPORTED_API) {
    return `该模型使用 ${compat.api} 协议，当前版本只能识别、不能调用（仅 OpenAI 兼容协议可用）`;
  }
  if (compat.reasoning === false) return "预设模型无思考模式，无需关思考参数";
  if (compat.thinkingOffKind === "none") {
    return "预设未提供关闭思考的参数（该模型可能默认思考且无法关闭；翻译可能失败，可换模型）";
  }
  const kind = compat.thinkingOffKind || "未知（自定义端点靠验证探测）";
  return `关思考参数：${kind}${compat.thinkingFormat ? `（pi-ai ${compat.thinkingFormat} 格式）` : ""}；验证策略：${strategy || "auto"}`;
}

/** 上下文窗口（K/M 缩写） */
export function contextLabel(tokens: number): string {
  if (!tokens) return "?";
  return tokens >= 1_000_000 ? `${Math.round(tokens / 100_000) / 10}M` : `${Math.round(tokens / 1000)}K`;
}

/** 价格（美元/百万 token；免费 = 免费） */
export function costLabel(model: PiModel): string {
  if (!model.cost) return "免费";
  return `$${model.cost.in}/$${model.cost.out}`;
}
