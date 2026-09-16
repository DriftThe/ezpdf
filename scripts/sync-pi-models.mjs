// pi-ai 目录同步（用户 2026-09-15）：把 @mariozechner/pi-ai 的模型目录固化成仓库内的
// 前端数据模块 src/lib/piModels.generated.ts，作为「供应商 / 模型预设」的唯一数据源。
//
// 为什么不直接依赖 pi-ai：它把 openai / anthropic / google / mistral / aws-sdk / undici
// 等 SDK 全挂进 dependencies（前端 bundle 里用不上，也不适合塞进 Tauri webview），
// 而我们要的只是它的目录 + 兼容性判定（纯数据、零 import 的 models.generated.js）。
// 运行时（Rust translate.rs）仍是我们自己的 OpenAI 兼容客户端。
//
// 用法：node scripts/sync-pi-models.mjs [--version=0.73.1] [--latest] [--check]
//   --check 只校验仓库内文件与当前 pi-ai 版本是否一致（CI 用，不写文件）
//
// 同时把 pi-ai dist/providers/openai-completions.js 里**非导出**的 detectCompat/
// thinking 参数映射移植过来（见 DETECT_COMPAT_NOTE），版本升级后需重新核对。

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const cacheDir = path.join(root, "node_modules", ".cache", "pi-ai");
const outFile = path.join(root, "src", "lib", "piModels.generated.ts");

/** 与仓库内生成文件对齐的版本；--latest 时从 npm 解析 */
const PINNED_VERSION = "0.73.1";

/** Rust 客户端实现的三种协议（translate.rs 的 Protocol）：统计与注释用 */
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

// ---- pi-ai 兼容性判定移植 ----------------------------------------------------------------
// DETECT_COMPAT_NOTE：以下 detectCompat 逐行对应 pi-ai dist/providers/openai-completions.js
// 的 detectCompat()（该函数未导出，只能抄）。升级 pi-ai 版本后请重新比对。
// 结论只用于「OpenAI 兼容协议」的请求体适配：max_tokens 字段名 + 关思考参数形态。
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

/** pi-ai 的关思考参数形态（openai-completions buildParams 的那串 if/else 的等价物）。
 *  "off" 级别下 pi-ai 实际写入的字段 → 我们的 Rust 侧按同一张表施加。
 *
 *  协议不同、写法不同（用户 2026-09-16 接入 Messages / Responses）：
 *  - anthropic-messages：只有 `thinking: {type:"disabled"}` 一种写法（pi-ai 的
 *    anthropic provider 在 thinkingEnabled=false 时写这个，与目录里的 off 值无关）；
 *  - openai-responses：`reasoning.effort`，值取目录的 off（GPT-5 目录标 off: null =
 *    关不掉，此时不写任何参数——与 pi-ai 的 buildParams 判定一致）；
 *  - 其余（含 openai-completions）：沿用 detectCompat 的形态表。 */
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
      // openai：仅当模型给了 off 值才写 effort，否则 pi-ai 什么都不加
      return typeof offValue === "string"
        ? { kind: "reasoning_effort", value: offValue }
        : { kind: "none" };
  }
}

/** env-api-keys.js 的 provider → API Key 环境变量表（正则抽取，格式稳定） */
function extractEnvKeys(src) {
  const map = {};
  const body = /const envMap = \{([\s\S]*?)\};/.exec(src)?.[1] ?? "";
  for (const m of body.matchAll(/(?:"([^"]+)"|([A-Za-z][\w-]*))\s*:\s*"([^"]+)"/g)) {
    map[m[1] ?? m[2]] = m[3];
  }
  return map;
}

// ---- 生成 -------------------------------------------------------------------------------

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
        thinkingFormat: compat.thinkingFormat,
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
  /** pi-ai 的关思考参数形态（openai | openrouter | deepseek | zai | qwen | qwen-chat-template） */
  thinkingFormat: string;
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
