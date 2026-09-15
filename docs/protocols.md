# 协议支持现状 / Protocol support

简体中文 | [English](#english)

## 为什么只支持 OpenAI 兼容协议

ezpdf 的翻译客户端（`src-tauri/src/translate.rs`）只实现了一族线协议：

- 请求：`POST {baseUrl}/chat/completions`，`messages: [{role, content}]`，从 `choices[].message.content` 取回复；
- 关思考：按端点/预设注入 `reasoning_effort` 等参数（见 AGENTS.md）；
- 模型列表：`GET {baseUrl}/models`。

pi-ai 目录为每个模型标注了它所属厂商的线协议（`api` 字段）。当前目录中的分布（按模型数）：

| `api` 取值 | 模型数 | 本客户端 |
| --- | --- | --- |
| `openai-completions` | 405 | 支持 |
| `anthropic-messages` | 263 | 不支持 |
| `bedrock-converse-stream` | 93 | 不支持 |
| `openai-responses` | 86 | 不支持 |
| `azure-openai-responses` | 42 | 不支持 |
| `google-generative-ai` | 29 | 不支持 |
| `mistral-conversations` | 28 | 不支持 |
| `google-vertex` | 13 | 不支持 |
| `openai-codex-responses` | 10 | 不支持 |

不支持的三种原因（不是「懒得做」，而是三处结构都不一样）：

1. **请求体结构不同**：system 提示词的位置（Anthropic 是顶层 `system` 字段，Google 是 `systemInstruction`）、内容块模型（Anthropic 为 `content: [{type:"text", text}]`）、工具调用表示法都不同。
2. **响应结构不同**：OpenAI 是 SSE `choices[].delta.content`；Anthropic 是 `content_block_delta`；Google 是 `candidates[].content.parts[]`。流式解析与非流式取字段各要一套。
3. **认证与额外头不同**：Anthropic 用 `x-api-key` + `anthropic-version`；Google 用 `x-goog-api-key`（或 `?key=`）；Bedrock 走 SigV4 签名（需要 AWS 凭据链，不只是 API Key）。

## 当前的处理方式

- **前端不列出这类供应商**（避免「选了才发现不能调用」）：设置 → LLM 的供应商下拉只列 `openai-completions` 的预设，外加「自定义（手填 Base URL / 模型）」。
- 旧配置若正指向这类供应商，仍会显示为一个**禁用项**并标注不可用；调用时 Rust 会直接拒绝并说明仅支持 OpenAI 兼容端点。
- 自定义端点只要实现 OpenAI Chat Completions 协议即可直接使用。

## 想接入新协议

在 `translate.rs` 的 `chat_request` 中按 `api` 值分支：构造请求体、解析响应体（可不做流式），并复用现有的关思考策略、重试与纠正逻辑。约每个协议半天到一天，尚未排期。

---

<a id="english"></a>

## English

The translation client (`src-tauri/src/translate.rs`) implements one wire protocol only: OpenAI Chat Completions (`POST {baseUrl}/chat/completions`, `messages: [{role, content}]`, reply at `choices[].message.content`), plus `GET {baseUrl}/models` for listings.

The pi-ai catalog tags every model with the vendor protocol it speaks (`api`). Current distribution by model count:

| `api` | Models | This client |
| --- | --- | --- |
| `openai-completions` | 405 | supported |
| `anthropic-messages` | 263 | not supported |
| `bedrock-converse-stream` | 93 | not supported |
| `openai-responses` | 86 | not supported |
| `azure-openai-responses` | 42 | not supported |
| `google-generative-ai` | 29 | not supported |
| `mistral-conversations` | 28 | not supported |
| `google-vertex` | 13 | not supported |
| `openai-codex-responses` | 10 | not supported |

Three structural differences stand in the way:

1. **Different request shape** — where the system prompt goes (a top-level `system` field for Anthropic, `systemInstruction` for Google), how content blocks are modelled (`content: [{type:"text", text}]` for Anthropic), and how tool calls are represented.
2. **Different response shape** — OpenAI streams `choices[].delta.content`, Anthropic emits `content_block_delta`, Google nests `candidates[].content.parts[]`; each needs its own parser for both streaming and non-streaming reads.
3. **Different auth and headers** — Anthropic needs `x-api-key` plus `anthropic-version`, Google uses `x-goog-api-key` (or `?key=`), and Bedrock requires SigV4 signing over an AWS credential chain rather than a plain API key.

As a result, providers speaking those protocols are no longer listed in the provider dropdown (a saved config pointing at one is shown as a disabled entry with a warning, and the backend rejects the call with a clear message). Any endpoint that implements OpenAI Chat Completions works as-is via the custom option.

Adding a protocol means branching on the `api` value inside `chat_request`: build the request body, parse the response body (streaming optional) and reuse the existing thinking-off, retry and correction logic — roughly half a day to a day per protocol. Not scheduled yet.
