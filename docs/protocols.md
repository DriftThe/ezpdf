# 协议支持现状 / Protocol support

简体中文 | [English](#english)

## 支持的三种线协议

翻译客户端（`src-tauri/src/translate.rs`）实现三种协议，`api` 字段（pi-ai 目录的标注，
随设置持久化成 `llm.preset.api`）决定用哪一种：

| `api` | 协议 | 请求 | 认证 | 回复取字段 |
| --- | --- | --- | --- | --- |
| `openai-completions` | Chat Completions | `POST {base}/chat/completions`，`messages: [{role, content}]` | `Authorization: Bearer` | `choices[0].message.content` |
| `anthropic-messages` | Messages | `POST {base}/v1/messages`，`system` 顶层 + `messages` 数组 | `x-api-key` + `anthropic-version: 2023-06-01` | `content[]` 里 `type: "text"` 的块 |
| `openai-responses` | Responses | `POST {base}/responses`，`instructions` + `input[]` 项 | `Authorization: Bearer` | `output[]` 里 `type: "message"` 的 `output_text` |

流程（送翻块打包、上下文协议、纠正重试、关思考）三份协议完全共用，只有
「请求体形状 / 认证头 / 回复取字段」三处分叉，都收在 `Protocol` 的三个小函数里。
`""`（旧配置没这个键）按 Chat Completions 处理。

另外两处按协议分组：

- **关思考**：Chat 保持原有矩阵（`reasoning`/`enable_thinking`/`thinking_type`/
  `chat_template_kwargs`/`reasoning_effort`，见 AGENTS.md）；Messages 只有
  `thinking: {type: "disabled"}` 一种写法；Responses 写 `reasoning.effort`（目录给了
  off 值就用它，否则只在显式策略下写 `none` —— GPT-5.1+ 支持，更早的推理模型目录标
  `off: null` 即关不掉）。两种新协议下，若模型可能仍在思考就不写 `temperature`：这两个
  API 都会因为「温度 + 思考」同时出现而报错。
- **模型列表**：`GET {base}/models`；Messages 是 `GET {base}/v1/models`（同一个
  `x-api-key`）。Messages 的 Base URL 按 Anthropic SDK 约定不带版本段
  （`https://api.anthropic.com` → `/v1/messages`），也容忍已经带了 `/v1` 甚至完整
  `/v1/messages` 的写法。

## 仍未支持

| `api` 取值 | 模型数 | 原因 |
| --- | --- | --- |
| `bedrock-converse-stream` | 93 | 走 SigV4 签名 + AWS 凭据链，不只是 API Key |
| `azure-openai-responses` | 42 | 同 Responses 但 base URL 带 deployment、认证头是 `api-key` |
| `google-generative-ai` | 29 | `systemInstruction` + `candidates[].content.parts[]`，另有 `x-goog-api-key` |
| `mistral-conversations` | 28 | Mistral 自家的对话协议（工具/回复形状都不同） |
| `google-vertex` | 13 | 同 Google + GCP 认证 |
| `openai-codex-responses` | 10 | ChatGPT 后端专用（OAuth + 特殊头） |

这些供应商**不在设置页的供应商下拉里列出**（避免「选了才发现不能用」）；旧配置如果
正指向其中之一，会显示为一个禁用项并标注不可用，调用时 Rust 报错并说明支持哪三种。

## 想再接入一种协议

在 `translate.rs` 的 `Protocol` 上加一个分支即可：`chat_url`/`models_url`（路径）、
`build_body`（请求体 + 认证头）、`completion_of`（取文本 + 判断是否在思考），再按需
扩展关思考与 `verify_llm` 的候选策略。提示词与调度链路不用动。

---

<a id="english"></a>

## English

The translation client (`src-tauri/src/translate.rs`) speaks three wire protocols; the `api`
field (pi-ai's per-model tag, persisted as `llm.preset.api`) picks one:

| `api` | Protocol | Request | Auth | Reply |
| --- | --- | --- | --- | --- |
| `openai-completions` | Chat Completions | `POST {base}/chat/completions`, `messages: [{role, content}]` | `Authorization: Bearer` | `choices[0].message.content` |
| `anthropic-messages` | Messages | `POST {base}/v1/messages`, top-level `system` + `messages[]` | `x-api-key` + `anthropic-version: 2023-06-01` | `text` blocks inside `content[]` |
| `openai-responses` | Responses | `POST {base}/responses`, `instructions` + `input[]` items | `Authorization: Bearer` | `output_text` parts of `output[]` message items |

Everything else — block packing, the context protocol, correction retries, thinking-off — is
shared; only the request shape, the auth headers and the reply extraction branch, and those live
in three small functions behind `Protocol`. An empty `api` (old configs) means Chat Completions.

Two more spots are protocol-aware:

- **Thinking-off** — Chat keeps its matrix (`reasoning`/`enable_thinking`/`thinking_type`/
  `chat_template_kwargs`/`reasoning_effort`, see AGENTS.md); Messages only has
  `thinking: {type: "disabled"}`; Responses writes `reasoning.effort` (the catalog's off value
  when present, otherwise `none` only under an explicit strategy — GPT-5.1+ accepts it, older
  reasoning models are tagged `off: null`, i.e. cannot be disabled). Under both new protocols
  `temperature` is omitted while the model may still be thinking, since both APIs reject a
  temperature combined with an active reasoning mode.
- **Model listing** — `GET {base}/models`; Anthropic uses `GET {base}/v1/models` with the same
  `x-api-key`. A Messages base URL follows the Anthropic SDK convention (no version segment:
  `https://api.anthropic.com` → `/v1/messages`) and also tolerates a base that already ends in
  `/v1` or the full `/v1/messages`.

Still unsupported (and therefore **not listed** in the provider dropdown; a saved config pointing
at one shows as a disabled entry, and the backend rejects the call with a message naming the three
supported protocols): `bedrock-converse-stream` (93 models, SigV4 + AWS credential chain),
`azure-openai-responses` (42, deployment-based URL and an `api-key` header),
`google-generative-ai` (29, `systemInstruction` + `candidates[].content.parts[]`),
`mistral-conversations` (28), `google-vertex` (13), `openai-codex-responses` (10, OAuth backend).

Adding another protocol means one more branch in `Protocol`: `chat_url`/`models_url` (paths),
`build_body` (request body + auth headers) and `completion_of` (text + thinking detection), plus
whatever thinking-off and `verify_llm` candidates it needs. Prompts and the scheduling pipeline
stay untouched.
