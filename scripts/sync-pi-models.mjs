// Vendor the @mariozechner/pi-ai model catalog into src/lib/piModels.generated.ts, the single data
// source for provider/model presets. Not a runtime dependency: pi-ai pulls every SDK into the
// bundle, but we only need its pure-data catalog + compat detection (runtime translation stays in
// Rust translate.rs). Usage: node scripts/sync-pi-models.mjs [--version=X] [--latest] [--check];
// --check verifies the repo file against the current pi-ai version (CI, no write).

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const cacheDir = path.join(root, "node_modules", ".cache", "pi-ai");
const outFile = path.join(root, "src", "lib", "piModels.generated.ts");

/** Version aligned with the generated repo file; --latest resolves it from npm */
const PINNED_VERSION = "0.73.1";

/** The three protocols the Rust client implements (translate.rs Protocol); stats/comments only */
const SUPPORTED_APIS = ["openai-completions", "anthropic-messages", "openai-responses"];

const args = process.argv.slice(2);
const checkOnly = args.includes("--check");
const useLatest = args.includes("--latest");
const versionArg = args.find((a) => a.startsWith("--version="))?.slice("--version=".length);

const TAR =
  process.platform === "win32" && fs.existsSync("C:\\Windows\\System32\\tar.exe")
    ? "C:\\Windows\\System32\\tar.exe"
    : "tar";

function log(msg) {
  console.log(`[pi-models] ${msg}`);
}

async function resolveVersion() {
  if (versionArg) return versionArg;
  if (useLatest) {
    const resp = await fetch("https://registry.npmjs.org/@mariozechner/pi-ai/latest");
    if (!resp.ok) throw new Error(`查询 npm 版本失败: HTTP ${resp.status}`);
    return (await resp.json()).version;
  }
  return PINNED_VERSION;
}

async function fetchPackage(version) {
  fs.mkdirSync(cacheDir, { recursive: true });
  const dir = path.join(cacheDir, version);
  const marker = path.join(dir, "package", "dist", "models.generated.js");
  if (fs.existsSync(marker)) return dir;
  const tgz = path.join(cacheDir, `pi-ai-${version}.tgz`);
  if (!fs.existsSync(tgz)) {
    const url = `https://registry.npmjs.org/@mariozechner/pi-ai/-/pi-ai-${version}.tgz`;
    log(`下载 ${url}`);
    const resp = await fetch(url);
    if (!resp.ok) throw new Error(`下载失败: HTTP ${resp.status}`);
    fs.writeFileSync(tgz, Buffer.from(await resp.arrayBuffer()));
  }
  fs.mkdirSync(dir, { recursive: true });
  const r = spawnSync(TAR, ["-xzf", tgz, "-C", dir], { stdio: "inherit" });
  if (r.status !== 0) throw new Error("tar 解压失败");
  return dir;
}

// DETECT_COMPAT_NOTE: detectCompat mirrors pi-ai's non-exported openai-completions.js (copied, not
// exported); re-diff on upgrade. Drives request shaping only: max_tokens field + thinking-off shape.
function detectCompat(model) {
  const provider = model.provider;
  const baseUrl = model.baseUrl;
  const isZai = provider === "zai" || baseUrl.includes("api.z.ai");
  const isMoonshot = provider === "moonshotai" || provider === "moonshotai-cn" || baseUrl.includes("api.moonshot.");
  const isCloudflareAiGateway = provider === "cloudflare-ai-gateway" || baseUrl.includes("gateway.ai.cloudflare.com");
  const isDeepSeek = provider === "deepseek" || baseUrl.includes("deepseek.com");
  const useMaxTokens = baseUrl.includes("chutes.ai") || isMoonshot || isCloudflareAiGateway;
  return {
    maxTokensField: useMaxTokens ? "max_tokens" : "max_completion_tokens",
    thinkingFormat: isDeepSeek
      ? "deepseek"
      : isZai
        ? "zai"
        : provider === "openrouter" || baseUrl.includes("openrouter.ai")
          ? "openrouter"
          : "openai",
  };
}

/** pi-ai thinking-off parameter shapes (equivalent of openai-completions buildParams); Rust applies
 *  the same table. anthropic-messages → thinking.type only; openai-responses → reasoning.effort
 *  from the catalog off (off: null = cannot disable); everything else → the detectCompat shape. */
function thinkingOffShape(api, reasoning, format, offValue) {
  if (api === "anthropic-messages") {
    return { kind: reasoning ? "thinking_type" : "none" };
  }
  switch (format) {
    case "deepseek":
      return { kind: "thinking_type" };
    case "zai":
    case "qwen":
      return { kind: "enable_thinking" };
    case "qwen-chat-template":
      return { kind: "chat_template_kwargs" };
    case "openrouter":
      return { kind: "reasoning_effort", value: offValue ?? "none" };
    default:
      // openai: write effort only when the model provides an off value, else pi-ai adds nothing
      return typeof offValue === "string"
        ? { kind: "reasoning_effort", value: offValue }
        : { kind: "none" };
  }
}

/** env-api-keys.js provider -> API key env var table (regex-extracted, stable format) */
function extractEnvKeys(src) {
  const map = {};
  const body = /const envMap = \{([\s\S]*?)\};/.exec(src)?.[1] ?? "";
  for (const m of body.matchAll(/(?:"([^"]+)"|([A-Za-z][\w-]*))\s*:\s*"([^"]+)"/g)) {
    map[m[1] ?? m[2]] = m[3];
  }
  return map;
}


function toCatalog(MODELS, envKeys, version) {
  const providers = [];
  for (const providerId of Object.keys(MODELS)) {
    const models = [];
    const baseUrlCount = new Map();
    for (const model of Object.values(MODELS[providerId])) {
      const compat = { ...detectCompat(model), ...(model.compat ?? {}) };
      const offValue = model.thinkingLevelMap?.off ?? null;
      const shape = thinkingOffShape(model.api, model.reasoning, compat.thinkingFormat, offValue);
      baseUrlCount.set(model.baseUrl, (baseUrlCount.get(model.baseUrl) ?? 0) + 1);
      models.push({
        id: model.id,
        api: model.api,
        baseUrl: model.baseUrl,
        reasoning: model.reasoning,
        maxTokensField: compat.maxTokensField,
        thinkingOffKind: shape.kind,
        thinkingOffValue: shape.value ?? null,
        headers: model.headers ?? null,
      });
    }
    const baseUrls = [...baseUrlCount.entries()].sort((a, b) => b[1] - a[1]);
    providers.push({
      id: providerId,
      baseUrl: baseUrls[0]?.[0] ?? "",
      envKeys: envKeys[providerId] ? [envKeys[providerId]] : [],
      models: models.sort((a, b) => a.id.localeCompare(b.id)),
    });
  }
  return { version, providers: providers.sort((a, b) => a.id.localeCompare(b.id)) };
}

function render(catalog) {
  const stats = {
    providers: catalog.providers.length,
    models: catalog.providers.reduce((n, p) => n + p.models.length, 0),
    usable: catalog.providers.reduce((n, p) => n + p.models.filter((m) => SUPPORTED_APIS.includes(m.api)).length, 0),
  };
  return `// 本文件由 scripts/sync-pi-models.mjs 生成，请勿手改。
// 数据源：@mariozechner/pi-ai@${catalog.version} 的 dist/models.generated.js（模型目录，
// 零 import 的纯数据）+ dist/env-api-keys.js（API Key 环境变量表）+ 移植的 detectCompat。
// 刷新：node scripts/sync-pi-models.mjs --latest
// 规模：${stats.providers} 个供应商 / ${stats.models} 个模型（其中 ${stats.usable} 个可用当前客户端直连：
// Chat Completions / Messages / Responses 三种协议，见 src-tauri/src/translate.rs）

export interface PiModel {
  id: string;
  /** pi-ai 线上协议：openai-completions / anthropic-messages / openai-responses 三种可直连 */
  api: string;
  baseUrl: string;
  /** 是否有思考模式（false = 无需关思考参数） */
  reasoning: boolean;
  /** 请求体里 max tokens 的字段名（pi-ai compat 判定） */
  maxTokensField: "max_tokens" | "max_completion_tokens";
  /** 我们施加关思考参数的方式（none = 预设表示无法通过参数关闭；按协议不同写法：
   *  openai-completions → 顶层 reasoning_effort 等、anthropic-messages → thinking.type、
   *  openai-responses → reasoning.effort） */
  thinkingOffKind: "none" | "thinking_type" | "enable_thinking" | "chat_template_kwargs" | "reasoning_effort";
  thinkingOffValue: string | null;
  /** 模型特有请求头（pi-ai 目录里少数模型有） */
  headers: Record<string, string> | null;
}

export interface PiProvider {
  id: string;
  /** 供应商默认 Base URL（同一 provider 可能有多个端点，取最常见的） */
  baseUrl: string;
  /** 该供应商的 API Key 环境变量名（pi-ai 约定） */
  envKeys: string[];
  models: PiModel[];
}

export const PI_AI_VERSION = ${JSON.stringify(catalog.version)};

export const PI_CATALOG: PiProvider[] = ${JSON.stringify(catalog.providers)};
`;
}

async function main() {
  const version = await resolveVersion();
  let pkgDir = await fetchPackage(version);
  const modelsPath = path.join(pkgDir, "package", "dist", "models.generated.js");
  const envKeysPath = path.join(pkgDir, "package", "dist", "env-api-keys.js");
  const { MODELS } = await import(pathToFileURL(modelsPath).href);
  const envKeys = extractEnvKeys(fs.readFileSync(envKeysPath, "utf8"));
  const catalog = toCatalog(MODELS, envKeys, version);
  const text = render(catalog);
  const current = fs.existsSync(outFile) ? fs.readFileSync(outFile, "utf8") : "";
  const upToDate = current === text;
  if (checkOnly) {
    if (!upToDate) {
      console.error(`[pi-models] ${path.relative(root, outFile)} 与 pi-ai@${version} 不一致，请运行 node scripts/sync-pi-models.mjs`);
      process.exit(1);
    }
    log(`校验通过（pi-ai@${version}）`);
    return;
  }
  if (upToDate) {
    log(`已是最新（pi-ai@${version}），未改动`);
    return;
  }
  fs.writeFileSync(outFile, text);
  const kb = (Buffer.byteLength(text) / 1024).toFixed(0);
  log(
    `写入 ${path.relative(root, outFile)}（${kb}KB）：${catalog.providers.length} 供应商 / ` +
      `${catalog.providers.reduce((n, p) => n + p.models.length, 0)} 模型`,
  );
}

main().catch((e) => {
  console.error(`[pi-models] 失败：${e.message}`);
  process.exit(1);
});
