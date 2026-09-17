//! LLM translation: single-page packing + smart-context agent loop + external cross-page binding.
//! Prompts (runtime `system_prompt/`; EZPDF_SYSTEM_PROMPT_DIR overrides): smart_context ⇒
//! intelli_context.md (protocols below, result key `A`), else standard_translate.md (key `content`).
//!
//! Smart protocols: A `{requests:[{index,Q}]}` → B `{result:[{index,A}],need_context:false}` or a
//! context request `need_context:"before"|"after"` → C `{type,requests:[{index,C}]}` → D `{result,external_index,external}`.
//! Exactly ONE context request is allowed; `external` binds onto the neighbour's candidate block (later skipped there).
//!
//! Bad JSON / missing index appends a correction and retries (MAX_ROUNDS); results are validated 1:1 against requests (A=null valid).

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use ts_rs::TS;

use crate::parse::truncate;
use crate::{BindDoc, PageInfo};

/// Config from invoke (dev fills from root auth.cfg); the pi-ai preset fields are frontend-computed, so Rust embeds no catalog.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub target_lang: String,
    /// Smart-context translation toggle (on by default, see LlmSection).
    #[serde(default)]
    pub smart_context: bool,
    /// Thinking-off strategy ("auto" | "reasoning" | "enable_thinking" | "thinking_type" | "none") from Verify; explicit outranks preset.
    #[serde(default)]
    pub thinking_off: String,
    /// Protocol ("openai-completions" default | "anthropic-messages" | "openai-responses"); "" = Chat, others hard-error.
    #[serde(default)]
    pub api: String,
    /// Preset-derived thinking-off shape ("" = unknown → fall back to endpoint URL rules).
    #[serde(default)]
    pub thinking_off_kind: String,
    /// reasoning_effort value (openrouter/openai shape; None = cannot disable thinking).
    #[serde(default)]
    pub thinking_off_value: Option<String>,
    /// max-tokens field name ("" = max_tokens; "max_completion_tokens" = pi-ai compat rename).
    #[serde(default)]
    pub max_tokens_field: String,
    /// Whether the preset model reasons (None = unknown); false = no thinking-off param needed.
    #[serde(default)]
    pub model_reasoning: Option<bool>,
    /// Model-specific request headers (a few catalog models have them).
    #[serde(default)]
    pub extra_headers: std::collections::BTreeMap<String, String>,
    /// Block types to translate (Settings → General); empty = built-in default, else only the listed; affects untranslated pages only.
    #[serde(default)]
    pub translate_types: Vec<String>,
    /// false = no LLM request: source content becomes the translation and the page is marked done (apply_bypass); missing key = true.
    #[serde(default = "translate_enabled_default")]
    pub translate_enabled: bool,
}

/// Default true: the bypass path runs only when translation is explicitly disabled.
fn translate_enabled_default() -> bool {
    true
}

/// Hand-written Default: derive would set translate_enabled=false, opposite of serde's default true (tests would bypass).
impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            target_lang: String::new(),
            smart_context: false,
            thinking_off: String::new(),
            api: String::new(),
            thinking_off_kind: String::new(),
            thinking_off_value: None,
            max_tokens_field: String::new(),
            model_reasoning: None,
            extra_headers: std::collections::BTreeMap::new(),
            translate_types: Vec::new(),
            translate_enabled: translate_enabled_default(),
        }
    }
}

/// opencode zen endpoints also need a session-routing header besides bearer.
fn is_opencode(url: &str) -> bool {
    url.contains("opencode.ai")
}

// ---- Wire protocols: Chat Completions / Anthropic Messages / OpenAI Responses ----
// The three differ only in body/auth/response field; the translation chain is shared.

/// OpenAI-compatible Chat Completions (default; also used when `api` is empty).
const API_CHAT: &str = "openai-completions";
/// Anthropic Messages (native Claude; `POST {base}/v1/messages`).
const API_MESSAGES: &str = "anthropic-messages";
/// OpenAI Responses (`POST {base}/responses`).
const API_RESPONSES: &str = "openai-responses";
/// Anthropic version header (@anthropic-ai/sdk default, required).
const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Protocol {
    Chat,
    Messages,
    Responses,
}

impl Protocol {
    fn from_api(api: &str, model: &str) -> Result<Self, String> {
        match api.trim() {
            "" | API_CHAT => Ok(Protocol::Chat),
            API_MESSAGES => Ok(Protocol::Messages),
            API_RESPONSES => Ok(Protocol::Responses),
            other => Err(format!(
                "model {model} uses the {other} protocol; supported protocols: {API_CHAT}, {API_MESSAGES}, {API_RESPONSES}"
            )),
        }
    }

    /// Anthropic base URLs carry no version segment; tolerate a pasted `/v1` or `/v1/messages` too.
    fn anthropic_base(base_url: &str) -> &str {
        let base = base_url.trim().trim_end_matches('/');
        let base = base.strip_suffix("/messages").unwrap_or(base);
        base.strip_suffix("/v1").unwrap_or(base)
    }

    fn chat_url(self, base_url: &str) -> String {
        let base = base_url.trim().trim_end_matches('/');
        match self {
            Protocol::Chat => format!("{base}/chat/completions"),
            Protocol::Messages => format!("{}/v1/messages", Self::anthropic_base(base_url)),
            Protocol::Responses => format!("{base}/responses"),
        }
    }

    /// Models URL: Anthropic is `/v1/models`; the other bases already include the version segment.
    fn models_url(self, base_url: &str) -> String {
        let base = base_url.trim().trim_end_matches('/');
        match self {
            Protocol::Messages => format!("{}/v1/models", Self::anthropic_base(base_url)),
            _ => format!("{base}/models"),
        }
    }
}

impl LlmConfig {
    /// Missing any of the three fields means unconfigured: translation is skipped (OCR unaffected).
    pub fn usable(&self) -> bool {
        !self.base_url.trim().is_empty()
            && !self.api_key.trim().is_empty()
            && !self.model.trim().is_empty()
    }
}

// ---- Translation log sink (shown live in the LLM settings pane; also visible in the dev terminal) ----

static LOG_SINK: OnceLock<AppHandle> = OnceLock::new();

/// Injected with the AppHandle during setup; unit tests / no GUI leaves it terminal-only.
pub fn init_log(app: &AppHandle) {
    let _ = LOG_SINK.set(app.clone());
}

/// Translation-chain log (ring-truncated by the frontend; never written to disk).
pub fn log(msg: impl AsRef<str>) {
    let msg = msg.as_ref();
    println!("[llm] {msg}");
    if let Some(app) = LOG_SINK.get() {
        let _ = app.emit("llm://log", msg.to_string());
    }
}

/// Default translatable types, kept in sync with src/lib/blocks.ts BLOCK_TYPE_OPTIONS; non-empty translate_types overrides.
const TRANSLATABLE_TYPES: &[&str] = &[
    "text",
    "paragraph_title",
    "doc_title",
    "abstract",
    "aside_text",
    "footnote",
    "footer",
    "vision_footnote",
    "figure_title",
    "content",
];

const CONTEXT_CANDIDATES: usize = 3;
/// Max LLM rounds per page: initial + context + two correction retries.
const MAX_ROUNDS: usize = 4;
const MAX_TOKENS: u32 = 8192;
const REQUEST_TIMEOUT_SECS: u64 = 180;

/// Prompt dir resolved once in setup (EZPDF_SYSTEM_PROMPT_DIR > production resource_dir > dev repo root); prod's bundle.resources isn't reachable via CARGO_MANIFEST_DIR.
static PROMPT_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Setup hook: resolve the prompt directory into a global (same pattern as pyenv's PyPaths).
pub fn init_prompt_dir(app: &AppHandle) {
    let dir = resolve_prompt_dir(app);
    println!("[ezpdf] system_prompt dir = {}", dir.display());
    let _ = PROMPT_DIR.set(dir);
}

fn pick_prompt_dir(env: Option<&str>, resource: Option<&std::path::Path>, dev: &std::path::Path) -> PathBuf {
    if let Some(dir) = env {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir.trim());
        }
    }
    if let Some(dir) = resource {
        return dir.to_path_buf();
    }
    dev.to_path_buf()
}

fn dev_prompt_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../system_prompt")
}

fn resolve_prompt_dir(app: &AppHandle) -> PathBuf {
    let env = std::env::var("EZPDF_SYSTEM_PROMPT_DIR").ok();
    #[cfg(dev)]
    {
        let _ = app; // dev branch has no use for the AppHandle
        pick_prompt_dir(env.as_deref(), None, &dev_prompt_dir())
    }
    #[cfg(not(dev))]
    {
        // Same convention as pyenv's server_root: bundle.resources targets are relative to the
        // resource root; also accept a resources/ subdir for custom packaging layouts.
        let resource = tauri::Manager::path(app)
            .resource_dir()
            .ok()
            .map(|res| [res.join("system_prompt"), res.join("resources").join("system_prompt")])
            .and_then(|candidates| {
                candidates.into_iter().find(|dir| dir.join("intelli_context.md").is_file())
            });
        pick_prompt_dir(env.as_deref(), resource.as_deref(), &dev_prompt_dir())
    }
}

fn prompt_path(smart: bool) -> PathBuf {
    let file = if smart {
        "intelli_context.md"
    } else {
        "standard_translate.md"
    };
    let dir = match PROMPT_DIR.get() {
        Some(dir) => dir.clone(),
        // Without setup (unit tests): fall back to env override / dev repo root
        None => pick_prompt_dir(std::env::var("EZPDF_SYSTEM_PROMPT_DIR").ok().as_deref(), None, &dev_prompt_dir()),
    };
    dir.join(file)
}

/// Read the system prompt and replace {{target_language}} (file chosen by smart_context).
pub fn load_system_prompt(cfg: &LlmConfig) -> Result<String, String> {
    let path = prompt_path(cfg.smart_context);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("failed to read translation prompt {}: {e}", path.display()))?;
    let lang = if cfg.target_lang.trim().is_empty() {
        "Simplified Chinese"
    } else {
        cfg.target_lang.trim()
    };
    let text = text.replace("{{target_language}}", lang);
    // Fallback: if an edited prompt lost the placeholder, append the target language so it always reaches the model.
    Ok(if text.contains(lang) {
        text
    } else {
        format!("{text}\n\n目标语言：{lang}。所有译文必须使用该语言。\n")
    })
}

fn is_translatable(cfg: &LlmConfig, kind: &str, content: &str) -> bool {
    let allowed = if cfg.translate_types.is_empty() {
        TRANSLATABLE_TYPES.contains(&kind)
    } else {
        cfg.translate_types.iter().any(|t| t == kind)
    };
    allowed && !content.trim().is_empty()
}

/// Pending request; q is a string for normal blocks or `{"table":[[...]]}` for tables (see table.rs).
#[derive(Clone)]
struct RequestItem {
    index: String,
    q: serde_json::Value,
    block_pos: usize,
    /// Table blocks: the grid used for this request (shape validation + persisted grid); None otherwise.
    table: Option<crate::table::TableGrid>,
}

/// Grid from the bound JSON or parsed now; None on failure (block not sent/covered).
fn grid_for(block: &crate::Block) -> Option<crate::table::TableGrid> {
    if block.kind != crate::table::TABLE {
        return None;
    }
    block.grid.clone().or_else(|| crate::table::parse_markup(&block.content))
}

/// Whether the page has a parsable, untranslated table (only if table is a translate type) — the backfill gate.
fn has_pending_tables(cfg: &LlmConfig, page: &PageInfo) -> bool {
    page.blocks.iter().any(|b| {
        b.kind == crate::table::TABLE
            && b.translation.is_none()
            && is_translatable(cfg, &b.kind, &b.content)
            && grid_for(b).is_some()
    })
}

/// Collect translatable blocks (allowed type, non-empty, translation == null); externally-bound blocks are skipped.
fn collect_requests(cfg: &LlmConfig, page: &PageInfo, only_tables: bool) -> Vec<RequestItem> {
    let mut items: Vec<RequestItem> = Vec::new();
    for (pos, b) in page.blocks.iter().enumerate() {
        if !is_translatable(cfg, &b.kind, &b.content) || b.translation.is_some() {
            continue;
        }
        // On a translated page only tables are resent; other types with translation == null would re-translate same-language blocks forever.
        if only_tables && b.kind != crate::table::TABLE {
            continue;
        }
        // Table with no parsable grid: skip the whole block (no tokens, no cover; page still marked translated)
        let (q, table) = if b.kind == crate::table::TABLE {
            let Some(grid) = grid_for(b) else { continue };
            (crate::table::payload(&grid), Some(grid))
        } else {
            (serde_json::Value::String(b.content.clone()), None)
        };
        items.push(RequestItem {
            index: items.len().to_string(),
            q,
            block_pos: pos,
            table,
        });
    }
    items
}

#[derive(Clone)]
struct ContextItem {
    index: String,
    c: String,
    block_pos: usize,
}

/// Direction → neighbour index (1-based: before = previous, after = next; out of range = None).
fn neighbor_index(page_index: u32, direction: &str) -> Option<u32> {
    match direction {
        "before" => (page_index > 1).then(|| page_index - 1),
        "after" => page_index.checked_add(1), // no panic on a malformed u32::MAX index
        _ => None,
    }
}

/// Neighbour candidates: before = last ≤N, after = first ≤N translatable blocks (translated included as reference).
fn context_candidates(cfg: &LlmConfig, doc: &BindDoc, neighbor: u32, direction: &str) -> Vec<ContextItem> {
    let Some(page) = doc.pages.iter().find(|p| p.index == neighbor) else {
        return Vec::new();
    };
    let mut picked: Vec<(usize, &crate::Block)> = page
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| is_translatable(cfg, &b.kind, &b.content))
        .collect();
    if picked.is_empty() {
        return Vec::new();
    }
    if picked.len() > CONTEXT_CANDIDATES {
        picked = if direction == "before" {
            let n = picked.len() - CONTEXT_CANDIDATES;
            picked.split_off(n)
        } else {
            picked.truncate(CONTEXT_CANDIDATES);
            picked
        };
    }
    picked
        .into_iter()
        .enumerate()
        .map(|(i, (pos, b))| ContextItem {
            index: i.to_string(),
            // Tables show the model the grid JSON (same shape as the payload), not the markup stream
            c: match grid_for(b) {
                Some(grid) if b.kind == crate::table::TABLE => crate::table::payload(&grid).to_string(),
                _ => b.content.clone(),
            },
            block_pos: pos,
        })
        .collect()
}

#[derive(Debug, Default)]
struct Reply {
    result: Vec<(String, Option<String>)>,
    /// "before" | "after" (None = no context requested).
    need_context: Option<String>,
    external: Option<String>,
    external_index: Option<String>,
}

/// Tolerant JSON extraction: strip ``` fences, take the first `{` to the last `}`.
fn extract_json(raw: &str) -> Option<String> {
    let mut s = raw.trim();
    if let Some(rest) = s.strip_prefix("```") {
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        s = match rest.rfind("```") {
            Some(end) => &rest[..end],
            None => rest,
        };
        s = s.trim();
    }
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end < start {
        return None;
    }
    Some(s[start..=end].to_string())
}

/// Tolerant index field: string as-is, number stringified; other types None.
fn index_str(v: Option<&Value>) -> Option<String> {
    match v? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn parse_reply(raw: &str) -> Option<Reply> {
    let v: Value = serde_json::from_str(&extract_json(raw)?).ok()?;
    let mut r = Reply::default();
    if let Some(arr) = v.get("result").and_then(|x| x.as_array()) {
        for item in arr {
            let idx = index_str(item.get("index"))?;
            // Translation key accepts both prompts: smart mode `A` / standard mode `content`
            let a = match item.get("A").or_else(|| item.get("content")) {
                Some(Value::String(s)) => Some(s.clone()),
                Some(Value::Null) | None => None,
                Some(other) => Some(other.to_string()),
            };
            r.result.push((idx, a));
        }
    }
    r.need_context = v
        .get("need_context")
        .and_then(|x| x.as_str())
        .filter(|s| *s == "before" || *s == "after")
        .map(String::from);
    r.external = v.get("external").and_then(|x| x.as_str()).map(String::from);
    r.external_index = index_str(v.get("external_index"));
    Some(r)
}

/// result must match requests strictly (count + every index); A=null valid (same language). A bad table shape fails the whole page.
fn validate_result(
    requests: &[RequestItem],
    reply: &Reply,
) -> Result<Vec<Option<String>>, String> {
    if reply.result.len() != requests.len() {
        return Err(format!(
            "result count mismatch: requested {} got {}",
            requests.len(),
            reply.result.len()
        ));
    }
    let mut out = Vec::with_capacity(requests.len());
    for req in requests {
        let raw = match reply.result.iter().find(|(i, _)| *i == req.index) {
            Some((_, a)) => a.clone(),
            None => return Err(format!("result missing index {}", req.index)),
        };
        match (&req.table, raw) {
            // Table with A=null: whole table already in the target language → keep null (renders source matrix)
            (Some(_), None) => out.push(None),
            (Some(grid), Some(text)) => {
                let value: Value = serde_json::from_str(&text)
                    .map_err(|e| format!("table result index {} is not JSON: {e}", req.index))?;
                let normalized = crate::table::validate_value(&value, &crate::table::row_lengths(grid))
                    .map_err(|e| format!("table result index {} invalid: {e}", req.index))?;
                out.push(Some(normalized));
            }
            (None, raw) => out.push(raw),
        }
    }
    Ok(out)
}

/// Write translations back (A=None stays null = render source). Tables also persist the grid; table A=None stores the SOURCE MATRIX as translation (terminal, else backfill keeps selecting it).
fn apply_result(page: &mut PageInfo, requests: &[RequestItem], values: &[Option<String>]) {
    for (req, val) in requests.iter().zip(values) {
        let Some(block) = page.blocks.get_mut(req.block_pos) else { continue };
        if let Some(grid) = req.table.as_ref() {
            if block.grid.is_none() {
                block.grid = Some(grid.clone());
            }
        }
        match (req.table.as_ref(), val) {
            (Some(grid), None) => block.translation = Some(crate::table::source_matrix(grid)),
            (_, None) => {}
            (_, Some(v)) => block.translation = Some(v.clone()),
        }
    }
}

/// Resolve external against this round's C candidates → (block pos, translation); no match → None (context boxes untouched).
fn resolve_external(candidates: &[ContextItem], reply: &Reply) -> Option<(usize, String)> {
    let idx = reply.external_index.as_ref()?;
    let ext = reply.external.as_ref()?;
    let cand = candidates.iter().find(|c| c.index == *idx)?;
    Some((cand.block_pos, ext.clone()))
}

pub fn llm_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("failed to create LLM HTTP client: {e}"))
}

/// opencode zen needs this session-routing header (any non-empty value); others ignore it. Sent only for opencode.ai.
const OPENCODE_SESSION: &str = "ezpdf";

/// Thinking-off precedence: explicit Verify strategy > hard-coded endpoint rules (zen/SiliconFlow) > pi-ai preset shape; returns whether a param was written.
fn apply_thinking_off_chat(body: &mut Value, url: &str, cfg: &LlmConfig) -> bool {
    match cfg.thinking_off.trim() {
        "reasoning" => {
            body["reasoning"] = json!({"enabled": false});
            body["reasoning_effort"] = json!("none");
            return true;
        }
        "enable_thinking" => {
            body["enable_thinking"] = json!(false);
            return true;
        }
        "thinking_type" => {
            body["thinking"] = json!({"type": "disabled"});
            return true;
        }
        "none" => return false,
        _ => {}
    }
    // Preset marks a non-reasoning model: write no thinking params, same as pi-ai
    if cfg.model_reasoning == Some(false) {
        return false;
    }
    if is_opencode(&url) {
        body["reasoning"] = json!({"enabled": false});
        body["reasoning_effort"] = json!("none");
        return true;
    }
    if url.contains("siliconflow") {
        // SiliconFlow (Qwen3.5 has thinking on by default)
        body["enable_thinking"] = json!(false);
        body["thinking"] = json!({"type": "disabled"});
        return true;
    }
    match cfg.thinking_off_kind.trim() {
        "thinking_type" => {
            body["thinking"] = json!({"type": "disabled"});
            true
        }
        "enable_thinking" => {
            body["enable_thinking"] = json!(false);
            true
        }
        "chat_template_kwargs" => {
            body["chat_template_kwargs"] = json!({"enable_thinking": false, "preserve_thinking": true});
            true
        }
        "reasoning_effort" => {
            let value = cfg.thinking_off_value.clone().unwrap_or_else(|| "none".into());
            body["reasoning_effort"] = json!(value);
            true
        }
        _ => false,
    }
}

/// Messages: only `thinking.type=disabled`; thinking is opt-in, so non-reasoning models and "none" write nothing.
fn apply_thinking_off_messages(body: &mut Value, cfg: &LlmConfig) -> bool {
    if cfg.thinking_off.trim() == "none" || cfg.model_reasoning == Some(false) {
        return false;
    }
    body["thinking"] = json!({"type": "disabled"});
    true
}

/// Responses: nested `reasoning.effort` (unlike Chat's top-level); catalog off value wins, else explicit writes "none"; older `off: null` models write nothing.
fn apply_thinking_off_responses(body: &mut Value, cfg: &LlmConfig) -> bool {
    if cfg.thinking_off.trim() == "none" || cfg.model_reasoning == Some(false) {
        return false;
    }
    let explicit = matches!(cfg.thinking_off.trim(), "reasoning" | "reasoning_effort");
    let effort = cfg
        .thinking_off_value
        .clone()
        .or_else(|| explicit.then(|| "none".to_string()));
    match effort {
        Some(effort) => {
            body["reasoning"] = json!({ "effort": effort });
            true
        }
        None => false,
    }
}

/// Still-reasoning (preset yes, no off param): under Messages/Responses temperature conflicts with thinking, so omit it.
fn may_still_think(cfg: &LlmConfig, off_applied: bool) -> bool {
    cfg.model_reasoning == Some(true) && !off_applied
}

/// Pull the system prompt out (Messages: top-level system; Responses: instructions) and normalize the rest to {role, content}.
fn split_system(messages: &[Value]) -> (Option<String>, Vec<Value>) {
    let mut system: Option<String> = None;
    let mut rest = Vec::new();
    for msg in messages {
        let role = msg["role"].as_str().unwrap_or_default();
        let text = msg["content"].as_str().unwrap_or_default().to_string();
        if role == "system" {
            system = Some(match system {
                Some(prev) => format!("{prev}\n{text}"),
                None => text,
            });
        } else {
            rest.push(json!({ "role": role, "content": text }));
        }
    }
    (system, rest)
}

/// Responses input items: user = input_text, assistant = output_text; the system prompt is skipped (already in instructions).
fn responses_input(messages: &[Value]) -> Vec<Value> {
    let mut input = Vec::new();
    for msg in messages {
        let role = msg["role"].as_str().unwrap_or_default();
        let kind = match role {
            "user" => "input_text",
            "assistant" => "output_text",
            _ => continue,
        };
        let text = msg["content"].as_str().unwrap_or_default();
        input.push(json!({ "role": role, "content": [{ "type": kind, "text": text }] }));
    }
    input
}

/// Build the body per protocol, returning whether a thinking-off param was written; max tokens: Chat preset field, Messages max_tokens, Responses max_output_tokens.
fn build_body(
    protocol: Protocol,
    cfg: &LlmConfig,
    messages: &[Value],
    max_tokens: u32,
    url: &str,
) -> (Value, bool) {
    match protocol {
        Protocol::Chat => {
            let token_field = if cfg.max_tokens_field.trim() == "max_completion_tokens" {
                "max_completion_tokens"
            } else {
                "max_tokens"
            };
            let mut body = json!({
                "model": cfg.model,
                "messages": messages,
                "temperature": 0,
                "stream": false,
            });
            body[token_field] = json!(max_tokens);
            let off = apply_thinking_off_chat(&mut body, url, cfg);
            (body, off)
        }
        Protocol::Messages => {
            let (system, rest) = split_system(messages);
            let mut body = json!({
                "model": cfg.model,
                "max_tokens": max_tokens,
                "messages": rest,
                "stream": false,
            });
            if let Some(system) = system {
                body["system"] = json!(system);
            }
            let off = apply_thinking_off_messages(&mut body, cfg);
            if !may_still_think(cfg, off) {
                body["temperature"] = json!(0);
            }
            (body, off)
        }
        Protocol::Responses => {
            let (system, _) = split_system(messages);
            let mut body = json!({
                "model": cfg.model,
                "input": responses_input(messages),
                "max_output_tokens": max_tokens,
                "stream": false,
                // Do not retain the translation on the server (official OpenAI field; pi-ai sends it too)
                "store": false,
            });
            if let Some(system) = system {
                body["instructions"] = json!(system);
            }
            let off = apply_thinking_off_responses(&mut body, cfg);
            if !may_still_think(cfg, off) {
                body["temperature"] = json!(0);
            }
            (body, off)
        }
    }
}

/// One chat request (protocol validation + body + auth + zen routing + extra headers), shared by translation and verification.
fn chat_request(
    client: &reqwest::Client,
    cfg: &LlmConfig,
    messages: &[Value],
    max_tokens: u32,
) -> Result<reqwest::RequestBuilder, String> {
    let protocol = Protocol::from_api(&cfg.api, &cfg.model)?;
    let url = protocol.chat_url(&cfg.base_url);
    let (body, _) = build_body(protocol, cfg, messages, max_tokens, &url);
    let mut req = match protocol {
        Protocol::Chat | Protocol::Responses => client.post(&url).bearer_auth(&cfg.api_key),
        // Anthropic uses x-api-key (not bearer) + the required version header
        Protocol::Messages => client
            .post(&url)
            .header("x-api-key", &cfg.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION),
    };
    if is_opencode(&url) {
        req = req.header("x-opencode-session", OPENCODE_SESSION);
    }
    for (key, value) in &cfg.extra_headers {
        req = req.header(key, value);
    }
    Ok(req.json(&body))
}

/// Protocol-agnostic view of one non-streaming reply (text + still-thinking), shared by verification and translation.
#[derive(Debug)]
struct Completion {
    /// Reply text (extracted differently per protocol; None = no text field in the response).
    text: Option<String>,
    /// Still thinking: a thinking block/item is present, or text is empty (thinking consumed max_tokens).
    thinks: bool,
}

fn completion_of(protocol: Protocol, body: &Value, raw: &str) -> Result<Completion, String> {
    match protocol {
        Protocol::Chat => {
            let msg = &body["choices"][0]["message"];
            if !msg.is_object() {
                return Err(format!(
                    "LLM response missing choices[0].message: {}",
                    truncate(raw, 200)
                ));
            }
            Ok(Completion {
                text: msg["content"].as_str().map(String::from),
                thinks: message_thinks(msg),
            })
        }
        Protocol::Messages => {
            let blocks = body["content"].as_array().ok_or_else(|| {
                format!("LLM response missing content[]: {}", truncate(raw, 200))
            })?;
            let text = blocks
                .iter()
                .filter(|b| b["type"].as_str() == Some("text"))
                .filter_map(|b| b["text"].as_str())
                .collect::<String>();
            // thinking / redacted_thinking blocks mean it is still reasoning
            let thinking = blocks.iter().any(|b| {
                matches!(b["type"].as_str(), Some("thinking") | Some("redacted_thinking"))
                    && b["thinking"].as_str().map(|t| !t.trim().is_empty()).unwrap_or(true)
            });
            Ok(Completion {
                text: Some(text.clone()),
                thinks: thinking || text.trim().is_empty(),
            })
        }
        Protocol::Responses => {
            let output = body["output"]
                .as_array()
                .ok_or_else(|| format!("LLM response missing output[]: {}", truncate(raw, 200)))?;
            let text = output
                .iter()
                .filter(|item| item["type"].as_str() == Some("message"))
                .flat_map(|item| item["content"].as_array().cloned().unwrap_or_default())
                .filter(|part| part["type"].as_str() == Some("output_text"))
                .map(|part| part["text"].as_str().unwrap_or_default().to_string())
                .collect::<String>();
            // a reasoning item means it is still reasoning
            let thinking = output.iter().any(|item| item["type"].as_str() == Some("reasoning"));
            Ok(Completion {
                text: Some(text.clone()),
                thinks: thinking || text.trim().is_empty(),
            })
        }
    }
}

async fn chat_raw(
    client: &reqwest::Client,
    cfg: &LlmConfig,
    messages: &[Value],
    max_tokens: u32,
) -> Result<Completion, String> {
    let protocol = Protocol::from_api(&cfg.api, &cfg.model)?;
    let resp = chat_request(client, cfg, messages, max_tokens)?
        .send()
        .await
        .map_err(|e| format!("LLM request failed: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("failed to read LLM response: {e}"))?;
    if !status.is_success() {
        return Err(format!("LLM returned {status}: {}", truncate(&text, 300)));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("LLM response is not JSON: {e}"))?;
    completion_of(protocol, &v, &text)
}

/// Non-streaming chat returning the reply text (wire details are absorbed in chat_raw).
async fn chat(client: &reqwest::Client, cfg: &LlmConfig, messages: &[Value]) -> Result<String, String> {
    let reply = chat_raw(client, cfg, messages, MAX_TOKENS).await?;
    reply
        .text
        .ok_or_else(|| "LLM response has no text content".to_string())
}

// ---- Settings Verify button: connectivity check + thinking-off strategy probe ----

/// Probe Q/A with a small max_tokens so a thinking model exhausts it on reasoning and returns empty content.
const PROBE_PROMPT: &str = "不要思考，直接回答：2+2 等于几？只输出数字。";
const PROBE_MAX_TOKENS: u32 = 64;

/// Verification report (shown in the LLM pane and written back as the thinkingOff strategy).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct LlmVerifyReport {
    /// Effective strategy; "none" = write nothing (unneeded or impossible, see thinking_on).
    pub strategy: String,
    /// Preset marks a non-reasoning model (thinking_on is always false then).
    pub preset_no_thinking: bool,
    /// The probe still produced reasoning: cannot be disabled (frontend toasts a warning).
    pub thinking_on: bool,
    pub message: String,
}

/// Whether a message still reasons: reasoning_content/reasoning non-empty, or content drained empty.
fn message_thinks(msg: &Value) -> bool {
    for key in ["reasoning_content", "reasoning"] {
        match msg.get(key) {
            Some(Value::String(s)) => {
                if !s.trim().is_empty() {
                    return true;
                }
            }
            Some(Value::Null) | None => {}
            Some(_) => return true, // object/array reasoning shape (OpenRouter)
        }
    }
    msg.get("content")
        .and_then(|c| c.as_str())
        .map(|c| c.trim().is_empty())
        .unwrap_or(true)
}

/// Verify connectivity + thinking-off strategy (first candidate that connects and stops reasoning wins); a non-reasoning preset probes connectivity only.
pub async fn verify_llm(cfg: &LlmConfig) -> Result<LlmVerifyReport, String> {
    if !cfg.usable() {
        return Err("please fill in Base URL / API Key / model first".into());
    }
    let protocol = Protocol::from_api(&cfg.api, &cfg.model)?;
    let client = llm_client()?;
    let messages = vec![json!({"role": "user", "content": PROBE_PROMPT})];
    let started = std::time::Instant::now();
    let preset_no_thinking = cfg.model_reasoning == Some(false);

    let ref_client = &client;
    let ref_messages = &messages;
    let probe = |strategy: &str| {
        let mut p = cfg.clone();
        p.thinking_off = strategy.to_string();
        async move { chat_raw(ref_client, &p, ref_messages, PROBE_MAX_TOKENS).await }
    };

    if preset_no_thinking {
        probe("auto").await?; // connectivity only: a non-reasoning model needs no strategy
        return Ok(LlmVerifyReport {
            strategy: "none".into(),
            preset_no_thinking,
            thinking_on: false,
            message: format!(
                "connection OK ({}ms); preset marks this as a non-reasoning model, no thinking-off parameter needed",
                started.elapsed().as_millis()
            ),
        });
    }

    // Candidates: Chat four field shapes; Messages thinking.type=disabled or nothing; Responses also explicit effort=none.
    let candidates: &[&str] = match protocol {
        Protocol::Chat => &["auto", "reasoning", "enable_thinking", "thinking_type"],
        Protocol::Messages => &["auto", "none"],
        Protocol::Responses => &["auto", "reasoning", "none"],
    };

    // Chat's four candidates run concurrently (serial was too slow); others serial short-circuit.
    let results: Vec<(&str, Result<Completion, String>)> = match protocol {
        Protocol::Chat => {
            let (auto, reasoning, enable_thinking, thinking_type) = tokio::join!(
                probe("auto"),
                probe("reasoning"),
                probe("enable_thinking"),
                probe("thinking_type"),
            );
            vec![
                ("auto", auto),
                ("reasoning", reasoning),
                ("enable_thinking", enable_thinking),
                ("thinking_type", thinking_type),
            ]
        }
        _ => {
            let mut out = Vec::new();
            for cand in candidates {
                let res = probe(cand).await;
                let ok = matches!(&res, Ok(c) if !c.thinks);
                out.push((*cand, res));
                if ok {
                    break; // thinking already off: skip the remaining candidates
                }
            }
            out
        }
    };
    let ms = started.elapsed().as_millis();
    let mut strategy = "";
    let mut first_err: Option<String> = None;
    let mut any_ok = false;
    for (cand, res) in results {
        match res {
            Ok(reply) => {
                any_ok = true;
                // First successful candidate that stopped thinking (a rejected param is a 4xx)
                if strategy.is_empty() && !reply.thinks {
                    strategy = cand;
                }
            }
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
    }
    if !any_ok {
        return Err(first_err.unwrap_or_else(|| "LLM probe failed".into()));
    }
    Ok(if strategy.is_empty() {
        LlmVerifyReport {
            strategy: "none".into(),
            preset_no_thinking,
            thinking_on: true,
            message: format!("connection OK ({ms}ms), but thinking mode could not be disabled after probing"),
        }
    } else {
        LlmVerifyReport {
            strategy: strategy.to_string(),
            preset_no_thinking,
            thinking_on: false,
            message: format!("connection OK ({ms}ms), thinking-off strategy: {strategy}"),
        }
    })
}

/// Fetch the model list (chat/responses use `/models`; Anthropic `/v1/models` + x-api-key).
pub async fn fetch_models(base_url: &str, api_key: &str, api: &str) -> Result<Vec<String>, String> {
    if base_url.trim().is_empty() || api_key.trim().is_empty() {
        return Err("please fill in Base URL / API Key first".into());
    }
    let protocol = Protocol::from_api(api, "")?;
    let client = llm_client()?;
    let url = protocol.models_url(base_url);
    let mut req = match protocol {
        Protocol::Messages => client
            .get(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", ANTHROPIC_VERSION),
        _ => client.get(&url).bearer_auth(api_key),
    };
    if is_opencode(&url) {
        req = req.header("x-opencode-session", OPENCODE_SESSION);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("failed to fetch model list: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("failed to fetch model list: {e}"))?;
    if !status.is_success() {
        return Err(format!("model list returned {status}: {}", truncate(&text, 200)));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("model list is not JSON: {e}"))?;
    let mut ids: Vec<String> = v["data"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m["id"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

const CORRECTION: &str = "上一轮输出无法解析或与 requests 不匹配。请仅输出合法 JSON，且 result 必须与本次输入 requests 的 index 一一对应（A 允许为 null，表示无需翻译）。";
const NO_MORE_CONTEXT: &str =
    "已提供过上下文；请立即输出最终译文（格式 B 或 D），不要再申请上下文。";

/// Single-page snapshot (concurrent tasks never touch &mut BindDoc); both before/after candidates are taken up front.
pub struct PageTask {
    page_index: u32,
    requests: Vec<RequestItem>,
    before: Option<(u32, Vec<ContextItem>)>,
    after: Option<(u32, Vec<ContextItem>)>,
}

/// Single-page result; the scheduler applies it to BindDoc under the lock.
pub struct PageDone {
    page_index: u32,
    requests: Vec<RequestItem>,
    values: Vec<Option<String>>,
    /// (neighbour index, neighbour block pos, translation): the external cross-page binding.
    external: Option<(u32, usize, String)>,
}

/// Doc snapshot → single-page task; missing page / not OCR-finished / translated → None.
pub fn build_task(cfg: &LlmConfig, doc: &BindDoc, page_index: u32) -> Option<PageTask> {
    let page = doc.pages.iter().find(|p| p.index == page_index)?;
    if !page.finished {
        return None;
    }
    // A translated page allows table-only backfill (other types untouched, so never re-translate holds); only parsable grids.
    let only_tables = page.translated;
    if only_tables && !has_pending_tables(cfg, page) {
        return None;
    }
    Some(PageTask {
        page_index,
        requests: collect_requests(cfg, page, only_tables),
        before: neighbor_index(page_index, "before")
            .map(|n| (n, context_candidates(cfg, doc, n, "before"))),
        after: neighbor_index(page_index, "after")
            .map(|n| (n, context_candidates(cfg, doc, n, "after"))),
    })
}

/// Bypass persistence (under the lock): copy translatable content into translation and mark done. Deliberately not null (null = "to translate" would re-run); tables store the SOURCE MATRIX + grid.
pub fn apply_bypass(doc: &mut BindDoc, page_index: u32, cfg: &LlmConfig) -> bool {
    let Some(page) = doc.pages.iter_mut().find(|p| p.index == page_index) else {
        return false;
    };
    if !page.finished || page.translated {
        return false;
    }
    for block in page.blocks.iter_mut() {
        if block.translation.is_some() || !is_translatable(cfg, &block.kind, &block.content) {
            continue;
        }
        if block.kind == crate::table::TABLE {
            let Some(grid) = grid_for(block) else { continue };
            if block.grid.is_none() {
                block.grid = Some(grid.clone());
            }
            block.translation = Some(crate::table::source_matrix(&grid));
        } else {
            block.translation = Some(block.content.clone());
        }
    }
    page.translated = true; // page-level progress: an all-untranslatable page must not be retried forever
    true
}

/// Persist under the lock: page translations + flag + external neighbour; touched collects changed pages for updatedPages.
pub fn apply_done(doc: &mut BindDoc, done: PageDone, touched: &mut Vec<u32>) {
    let PageDone { page_index, requests, values, external } = done;
    if let Some(page) = doc.pages.iter_mut().find(|p| p.index == page_index) {
        apply_result(page, &requests, &values);
        page.translated = true;
        touched.push(page_index);
    }
    if let Some((neighbor, pos, text)) = external {
        if let Some(page) = doc.pages.iter_mut().find(|p| p.index == neighbor) {
            if let Some(block) = page.blocks.get_mut(pos) {
                block.translation = Some(text);
                touched.push(neighbor);
            }
        }
    }
}

/// Build the format-C context reply (candidates only if the direction has a neighbour, else type null); returns (message, neighbour ctx).
fn context_reply(
    page_index: u32,
    round: usize,
    ms: u128,
    dir: &str,
    before: Option<&(u32, Vec<ContextItem>)>,
    after: Option<&(u32, Vec<ContextItem>)>,
) -> (String, Option<(u32, Vec<ContextItem>)>) {
    let snapshot = if dir == "before" { before } else { after };
    let (neighbor, candidates) = match snapshot {
        Some((n, c)) => (Some(*n), c.clone()),
        None => (None, Vec::new()),
    };
    let c = json!({
        "type": if candidates.is_empty() { Value::Null } else { json!(dir) },
        "requests": candidates
            .iter()
            .map(|c| json!({"index": c.index, "C": c.c}))
            .collect::<Vec<_>>(),
    })
    .to_string();
    match neighbor {
        Some(n) if candidates.is_empty() => log(format!(
            "p{page_index} round {} {ms}ms request {dir} context → p{n} no candidates, reply null",
            round + 1
        )),
        Some(n) => log(format!(
            "p{page_index} round {} {ms}ms request {dir} context → p{n} {} candidate blocks",
            round + 1,
            candidates.len()
        )),
        None => log(format!(
            "p{page_index} round {} {ms}ms request {dir} context → no candidates, reply null",
            round + 1
        )),
    }
    (c, neighbor.map(|n| (n, candidates)))
}

/// Translate one page (no doc dependency); empty requests → return empty so the caller marks it translated.
pub async fn translate_task(
    cfg: &LlmConfig,
    prompt: &str,
    client: &reqwest::Client,
    task: PageTask,
) -> Result<PageDone, String> {
    let PageTask { page_index, requests, before, after } = task;
    if requests.is_empty() {
        log(format!("p{page_index} no translatable blocks (empty text/already bound) → mark done"));
        return Ok(PageDone {
            page_index,
            requests,
            values: Vec::new(),
            external: None,
        });
    }
    log(format!("p{page_index} {} blocks queued", requests.len()));

    let user_a = json!({
        "requests": requests
            .iter()
            .map(|r| json!({"index": r.index, "Q": r.q}))
            .collect::<Vec<_>>(),
    })
    .to_string();
    let mut messages = vec![
        json!({"role": "system", "content": prompt}),
        json!({"role": "user", "content": user_a}),
    ];

    let mut context_answered = false; // a C (or null) reply was already sent
    // Context already sent (neighbour index + candidates): used for external persistence once the final output arrives
    let mut context: Option<(u32, Vec<ContextItem>)> = None;

    for _round in 0..MAX_ROUNDS {
        let started = std::time::Instant::now();
        let raw = chat(client, cfg, &messages).await?;
        let ms = started.elapsed().as_millis();
        let reply = match parse_reply(&raw) {
            Some(r) => r,
            None => {
                log(format!(
                    "p{page_index} round {} {ms}ms output unparsable → correction retry",
                    _round + 1
                ));
                messages.push(json!({"role": "assistant", "content": raw}));
                messages.push(json!({"role": "user", "content": CORRECTION}));
                continue;
            }
        };

        // Context request: smart mode only (standard mode has no such protocol; fall through to validation)
        if cfg.smart_context {
            if let Some(dir) = reply.need_context.clone() {
                if context_answered {
                    log(format!("p{page_index} round {} {ms}ms repeated context request → force final output", _round + 1));
                    messages.push(json!({"role": "assistant", "content": raw}));
                    messages.push(json!({"role": "user", "content": NO_MORE_CONTEXT}));
                    continue;
                }
                context_answered = true;
                let (c, ctx) = context_reply(
                    page_index,
                    _round,
                    ms,
                    &dir,
                    before.as_ref(),
                    after.as_ref(),
                );
                if ctx.is_some() {
                    context = ctx;
                }
                messages.push(json!({"role": "assistant", "content": raw}));
                messages.push(json!({"role": "user", "content": c}));
                continue;
            }
        }

        // Final output: validate → return
        let values = match validate_result(&requests, &reply) {
            Ok(v) => v,
            Err(e) => {
                log(format!(
                    "p{page_index} round {} {ms}ms result validation failed ({e}) → correction retry",
                    _round + 1
                ));
                messages.push(json!({"role": "assistant", "content": raw}));
                messages.push(json!({"role": "user", "content": format!("{CORRECTION}（问题：{e}）")}));
                continue;
            }
        };
        let nulls = values.iter().filter(|v| v.is_none()).count();
        log(format!(
            "p{page_index} done: {} translations (null {nulls})",
            values.len() - nulls
        ));
        let external = match context.as_ref() {
            Some((neighbor, candidates)) => match resolve_external(candidates, &reply) {
                Some((pos, text)) => {
                    log(format!(
                        "p{page_index} external write-back to p{neighbor} (candidate index {:?})",
                        reply.external_index
                    ));
                    Some((*neighbor, pos, text))
                }
                None => {
                    if reply.external_index.is_some() {
                        log(format!(
                            "p{page_index} external_index {:?} matched no candidate → ignore",
                            reply.external_index
                        ));
                    }
                    None
                }
            },
            None => None,
        };
        return Ok(PageDone {
            page_index,
            requests,
            values,
            external,
        });
    }
    Err(format!("page {page_index} translation failed: LLM output retries exhausted"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Block, PDFStatus};

    fn blk(kind: &str, content: &str, translation: Option<&str>) -> Block {
        Block {
            kind: kind.into(),
            content: content.into(),
            loc: [0.0; 4],
            translation: translation.map(String::from),
            grid: None,
        }
    }

    fn page(index: u32, blocks: Vec<Block>, translated: bool) -> PageInfo {
        PageInfo {
            index,
            finished: true,
            translated,
            blocks,
        }
    }

    fn doc(pages: Vec<PageInfo>) -> BindDoc {
        BindDoc {
            status: PDFStatus::Processing,
            pages,
        }
    }

    fn cfg(smart: bool) -> LlmConfig {
        LlmConfig {
            base_url: "http://x".into(),
            api_key: "k".into(),
            model: "m".into(),
            target_lang: "Simplified Chinese".into(),
            smart_context: smart,
            thinking_off: "auto".into(),
            ..Default::default()
        }
    }

    /// Inputs for thinking-off: explicit strategy + pi-ai preset shape.
    fn think_cfg(strategy: &str, kind: &str, value: Option<&str>, reasoning: Option<bool>) -> LlmConfig {
        LlmConfig {
            thinking_off: strategy.into(),
            thinking_off_kind: kind.into(),
            thinking_off_value: value.map(String::from),
            model_reasoning: reasoning,
            ..Default::default()
        }
    }

    #[test]
    fn prompt_dir_priority_env_resource_dev() {
        let dev = std::path::Path::new("/dev-repo/system_prompt");
        let res = std::path::Path::new("/usr/lib/ezpdf/system_prompt");
        // env override wins (trimmed)
        assert_eq!(pick_prompt_dir(Some("  /custom  "), Some(res), dev), PathBuf::from("/custom"));
        // then the installer resource dir (production)
        assert_eq!(pick_prompt_dir(None, Some(res), dev), res);
        // blank env counts as unset
        assert_eq!(pick_prompt_dir(Some("   "), Some(res), dev), res);
        // neither → dev repo root (unit tests / source tree)
        assert_eq!(pick_prompt_dir(None, None, dev), dev);
    }

    #[test]
    fn prompt_files_load_by_mode_and_template_lang() {
        let smart = cfg(true);
        let text = load_system_prompt(&smart).unwrap();
        assert!(text.contains("Simplified Chinese"));
        assert!(text.contains("need_context")); // smart protocol is in the prompt
        let plain = cfg(false);
        let text = load_system_prompt(&plain).unwrap();
        assert!(text.contains("Simplified Chinese"));
        assert!(!text.contains("need_context")); // standard prompt has no context protocol
        assert!(text.contains("\"content\""));
        // fallback appends the target language when the placeholder is gone
        let mut custom = cfg(false);
        custom.target_lang = "Klingon".into();
        let text = load_system_prompt(&custom).unwrap();
        assert!(text.contains("Klingon"));
    }

    #[test]
    fn thinking_off_strategies_shape_body() {
        let explicit = |s: &str| think_cfg(s, "", None, None);
        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://x/v1", &explicit("reasoning"));
        assert_eq!(body["reasoning"]["enabled"], json!(false));
        assert_eq!(body["reasoning_effort"], json!("none"));

        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://x/v1", &explicit("enable_thinking"));
        assert_eq!(body["enable_thinking"], json!(false));

        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://x/v1", &explicit("thinking_type"));
        assert_eq!(body["thinking"]["type"], json!("disabled"));

        // auto matrix (measured endpoint rules): zen → reasoning; siliconflow → enable_thinking+thinking
        let auto = |kind: &str, value: Option<&str>, reasoning: Option<bool>| {
            think_cfg("auto", kind, value, reasoning)
        };
        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://zen.opencode.ai/v1", &auto("", None, None));
        assert_eq!(body["reasoning"]["enabled"], json!(false));
        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://api.siliconflow.cn/v1", &auto("", None, None));
        assert_eq!(body["enable_thinking"], json!(false));
        assert_eq!(body["thinking"]["type"], json!("disabled"));

        // unknown endpoint writes nothing; explicit none too
        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://api.openai.com/v1", &auto("", None, None));
        assert!(body.as_object().unwrap().is_empty());
        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://api.openai.com/v1", &explicit("none"));
        assert!(body.as_object().unwrap().is_empty());

        // pi-ai preset shapes: deepseek / zai / qwen / openrouter / openai
        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://x/v1", &auto("thinking_type", None, Some(true)));
        assert_eq!(body["thinking"]["type"], json!("disabled"));
        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://x/v1", &auto("enable_thinking", None, Some(true)));
        assert_eq!(body["enable_thinking"], json!(false));
        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://x/v1", &auto("chat_template_kwargs", None, Some(true)));
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], json!(false));
        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://x/v1", &auto("reasoning_effort", None, Some(true)));
        assert_eq!(body["reasoning_effort"], json!("none")); // openrouter default off = "none"
        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://x/v1", &auto("reasoning_effort", Some("low"), Some(true)));
        assert_eq!(body["reasoning_effort"], json!("low"));
        // openai shape with no catalog off value → write nothing (same as pi-ai)
        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://x/v1", &auto("none", None, Some(true)));
        assert!(body.as_object().unwrap().is_empty());
        // preset marks non-reasoning → write nothing (even if the shape has a value)
        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://x/v1", &auto("enable_thinking", None, Some(false)));
        assert!(body.as_object().unwrap().is_empty());

        // unsupported protocol: the request fails before body construction
        let cfg = LlmConfig {
            base_url: "https://x/v1".into(),
            api_key: "k".into(),
            model: "gemini-3-pro".into(),
            api: "google-generative-ai".into(),
            ..Default::default()
        };
        let err = chat_request(&llm_client().unwrap(), &cfg, &[], 64).unwrap_err();
        assert!(err.contains("google-generative-ai"), "{err}");
    }

    /// Messages/Responses thinking-off: the former always writes thinking.type, the latter
    /// reasoning.effort (catalog value if present, else explicit only); non-reasoning writes nothing.
    #[test]
    fn thinking_off_shapes_follow_protocol() {
        let messages = |strategy: &str, reasoning: Option<bool>| think_cfg(strategy, "", None, reasoning);
        // Messages: auto and explicit field names collapse to the same write
        for strategy in ["auto", "thinking_type", "reasoning"] {
            let mut body = json!({});
            assert!(apply_thinking_off_messages(&mut body, &messages(strategy, Some(true))));
            assert_eq!(body["thinking"]["type"], json!("disabled"));
        }
        // Messages: explicit none / non-reasoning write nothing
        let mut body = json!({});
        assert!(!apply_thinking_off_messages(&mut body, &messages("none", Some(true))));
        let mut body = json!({});
        assert!(!apply_thinking_off_messages(&mut body, &messages("auto", Some(false))));
        assert!(body.as_object().unwrap().is_empty());

        // Responses: catalog off value wins; explicit strategy without one → "none"
        let mut body = json!({});
        assert!(apply_thinking_off_responses(&mut body, &think_cfg("auto", "reasoning_effort", Some("minimal"), Some(true))));
        assert_eq!(body["reasoning"]["effort"], json!("minimal"));
        let mut body = json!({});
        assert!(apply_thinking_off_responses(&mut body, &think_cfg("reasoning", "none", None, Some(true))));
        assert_eq!(body["reasoning"]["effort"], json!("none"));
        // Responses: auto with no catalog off value (GPT-5 marks off: null) → write nothing
        let mut body = json!({});
        assert!(!apply_thinking_off_responses(&mut body, &think_cfg("auto", "none", None, Some(true))));
        assert!(body.as_object().unwrap().is_empty());
        let mut body = json!({});
        assert!(!apply_thinking_off_responses(&mut body, &think_cfg("none", "reasoning_effort", Some("none"), Some(true))));
        assert!(body.as_object().unwrap().is_empty());
    }

    /// Request-body shapes for the three protocols: path / system location / max-tokens field /
    /// auth header / temperature vs thinking-mode exclusivity.
    #[test]
    fn request_bodies_follow_protocol() {
        let client = llm_client().unwrap();
        let messages = vec![
            json!({"role": "system", "content": "sys"}),
            json!({"role": "user", "content": "Q"}),
            json!({"role": "assistant", "content": "raw"}),
            json!({"role": "user", "content": "fix"}),
        ];
        let body_of = |cfg: &LlmConfig| -> Value {
            let req = chat_request(&client, cfg, &messages, 128).unwrap().build().unwrap();
            serde_json::from_slice(req.body().unwrap().as_bytes().unwrap()).unwrap()
        };

        // Chat: system stays in messages, max-tokens field follows the preset, bearer auth
        let chat = LlmConfig {
            base_url: "https://x/v1".into(),
            api_key: "k".into(),
            model: "m".into(),
            max_tokens_field: "max_completion_tokens".into(),
            ..Default::default()
        };
        let req = chat_request(&client, &chat, &messages, 128).unwrap().build().unwrap();
        assert_eq!(req.url().as_str(), "https://x/v1/chat/completions");
        assert_eq!(req.headers().get("authorization").unwrap(), "Bearer k");
        let body: Value = serde_json::from_slice(req.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(body["messages"][0]["role"], json!("system"));
        assert_eq!(body["max_completion_tokens"], json!(128));
        assert_eq!(body["temperature"], json!(0));

        // Messages: system goes top-level, non-system messages keep order, max_tokens required,
        // x-api-key + anthropic-version, URL gets /v1/messages
        let msg_cfg = LlmConfig {
            base_url: "https://api.anthropic.com".into(),
            api_key: "k".into(),
            model: "claude-sonnet-4-5".into(),
            api: API_MESSAGES.into(),
            model_reasoning: Some(true),
            ..Default::default()
        };
        let req = chat_request(&client, &msg_cfg, &messages, 128).unwrap().build().unwrap();
        assert_eq!(req.url().as_str(), "https://api.anthropic.com/v1/messages");
        assert_eq!(req.headers().get("x-api-key").unwrap(), "k");
        assert_eq!(req.headers().get("anthropic-version").unwrap(), ANTHROPIC_VERSION);
        assert!(req.headers().get("authorization").is_none());
        let body: Value = serde_json::from_slice(req.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(body["system"], json!("sys"));
        assert_eq!(body["max_tokens"], json!(128));
        assert_eq!(body["messages"].as_array().unwrap().len(), 3);
        assert_eq!(body["messages"][0]["role"], json!("user"));
        assert_eq!(body["messages"][2]["content"], json!("fix"));
        // thinking-off writes thinking.type, so temperature can be written
        assert_eq!(body["thinking"]["type"], json!("disabled"));
        assert_eq!(body["temperature"], json!(0));
        // same model + strategy "none": thinking off fails → omit temperature (Anthropic 400s)
        let no_off = LlmConfig { thinking_off: "none".into(), ..msg_cfg.clone() };
        assert!(body_of(&no_off).get("temperature").is_none());
        assert_eq!(body_of(&no_off)["thinking"], Value::Null);

        // Responses: instructions + input items (user=input_text / assistant=output_text),
        // max_output_tokens, bearer, URL gets /responses
        let resp_cfg = LlmConfig {
            base_url: "https://api.openai.com/v1".into(),
            api_key: "k".into(),
            model: "gpt-5.1".into(),
            api: API_RESPONSES.into(),
            model_reasoning: Some(true),
            ..Default::default()
        };
        let req = chat_request(&client, &resp_cfg, &messages, 128).unwrap().build().unwrap();
        assert_eq!(req.url().as_str(), "https://api.openai.com/v1/responses");
        assert_eq!(req.headers().get("authorization").unwrap(), "Bearer k");
        let body: Value = serde_json::from_slice(req.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(body["instructions"], json!("sys"));
        assert_eq!(body["max_output_tokens"], json!(128));
        assert_eq!(body["store"], json!(false));
        assert_eq!(body["input"][0]["role"], json!("user"));
        assert_eq!(body["input"][0]["content"][0]["type"], json!("input_text"));
        assert_eq!(body["input"][1]["content"][0]["type"], json!("output_text"));
        assert_eq!(body["input"].as_array().unwrap().len(), 3); // system is not in input
        // no catalog off value → cannot disable thinking → omit temperature (reasoning models reject it)
        assert!(body.get("temperature").is_none());
        assert_eq!(body_of(&LlmConfig { thinking_off: "reasoning".into(), ..resp_cfg.clone() })["reasoning"]["effort"], json!("none"));
    }

    /// Protocol mapping and URL normalization (custom endpoints often include /v1).
    #[test]
    fn protocol_from_api_and_urls() {
        assert_eq!(Protocol::from_api("", "m").unwrap(), Protocol::Chat);
        assert_eq!(Protocol::from_api(API_CHAT, "m").unwrap(), Protocol::Chat);
        assert_eq!(Protocol::from_api(" anthropic-messages ", "m").unwrap(), Protocol::Messages);
        assert_eq!(Protocol::from_api(API_RESPONSES, "m").unwrap(), Protocol::Responses);
        let err = Protocol::from_api("bedrock-converse-stream", "nova").unwrap_err();
        assert!(err.contains("bedrock-converse-stream") && err.contains("nova"), "{err}");

        let msgs = Protocol::Messages;
        assert_eq!(msgs.chat_url("https://api.anthropic.com"), "https://api.anthropic.com/v1/messages");
        assert_eq!(msgs.chat_url("https://api.anthropic.com/"), "https://api.anthropic.com/v1/messages");
        assert_eq!(msgs.chat_url("https://api.anthropic.com/v1"), "https://api.anthropic.com/v1/messages");
        assert_eq!(msgs.chat_url("https://api.anthropic.com/v1/messages"), "https://api.anthropic.com/v1/messages");
        assert_eq!(msgs.chat_url("https://api.minimax.io/anthropic"), "https://api.minimax.io/anthropic/v1/messages");
        assert_eq!(msgs.models_url("https://api.anthropic.com"), "https://api.anthropic.com/v1/models");
        assert_eq!(Protocol::Chat.chat_url("https://x/v1/"), "https://x/v1/chat/completions");
        assert_eq!(Protocol::Responses.chat_url("https://api.openai.com/v1"), "https://api.openai.com/v1/responses");
        assert_eq!(Protocol::Responses.models_url("https://api.openai.com/v1"), "https://api.openai.com/v1/models");
    }

    /// Response extraction: Chat=choices[0].message, Messages=content[] text blocks,
    /// Responses=output[] message/output_text; each protocol has its own thinking marker.
    #[test]
    fn completion_parsing_follows_protocol() {
        // Chat
        let chat = json!({"choices": [{"message": {"content": "4"}}]});
        let c = completion_of(Protocol::Chat, &chat, "{}").unwrap();
        assert_eq!(c.text.as_deref(), Some("4"));
        assert!(!c.thinks);
        let chat_think = json!({"choices": [{"message": {"content": "", "reasoning_content": "想"}}]});
        assert!(completion_of(Protocol::Chat, &chat_think, "{}").unwrap().thinks);
        assert!(completion_of(Protocol::Chat, &json!({}), "{}").unwrap_err().contains("choices[0].message"));

        // Messages: text blocks concatenated, a thinking block = still thinking
        let msgs = json!({"content": [{"type": "thinking", "thinking": "想…"}, {"type": "text", "text": "4"}]});
        let c = completion_of(Protocol::Messages, &msgs, "{}").unwrap();
        assert_eq!(c.text.as_deref(), Some("4"));
        assert!(c.thinks);
        let msgs_off = json!({"content": [{"type": "text", "text": "4"}]});
        assert!(!completion_of(Protocol::Messages, &msgs_off, "{}").unwrap().thinks);
        // only a thinking block (empty text) still counts as thinking
        let only_think = json!({"content": [{"type": "thinking", "thinking": "想"}]});
        assert!(completion_of(Protocol::Messages, &only_think, "{}").unwrap().thinks);
        assert!(completion_of(Protocol::Messages, &json!({}), "{}").unwrap_err().contains("content[]"));

        // Responses: a reasoning item = still thinking
        let resp = json!({"output": [
            {"type": "reasoning", "summary": [{"type": "summary_text", "text": "想"}]},
            {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "4"}]},
        ]});
        let c = completion_of(Protocol::Responses, &resp, "{}").unwrap();
        assert_eq!(c.text.as_deref(), Some("4"));
        assert!(c.thinks);
        let resp_off = json!({"output": [{"type": "message", "content": [{"type": "output_text", "text": "4"}]}]});
        assert!(!completion_of(Protocol::Responses, &resp_off, "{}").unwrap().thinks);
        // refusal parts are not text; empty text also counts as thinking
        let refusal = json!({"output": [{"type": "message", "content": [{"type": "refusal", "refusal": "no"}]}]});
        let c = completion_of(Protocol::Responses, &refusal, "{}").unwrap();
        assert_eq!(c.text.as_deref(), Some(""));
        assert!(c.thinks);
        assert!(completion_of(Protocol::Responses, &json!({}), "{}").unwrap_err().contains("output[]"));
    }

    #[test]
    fn max_tokens_field_follows_preset_compat() {
        let client = llm_client().unwrap();
        let cfg = LlmConfig {
            base_url: "https://x/v1".into(),
            api_key: "k".into(),
            model: "m".into(),
            max_tokens_field: "max_completion_tokens".into(),
            ..Default::default()
        };
        let req = chat_request(&client, &cfg, &[], 128).unwrap().build().unwrap();
        let body: Value = serde_json::from_slice(req.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(body["max_completion_tokens"], json!(128));
        assert!(body.get("max_tokens").is_none());

        let cfg = LlmConfig { max_tokens_field: "max_tokens".into(), ..cfg };
        let req = chat_request(&client, &cfg, &[], 128).unwrap().build().unwrap();
        let body: Value = serde_json::from_slice(req.body().unwrap().as_bytes().unwrap()).unwrap();
        assert_eq!(body["max_tokens"], json!(128));
    }

    #[test]
    fn extra_headers_reach_the_request() {
        let cfg = LlmConfig {
            base_url: "https://x/v1".into(),
            api_key: "k".into(),
            model: "m".into(),
            extra_headers: [("x-custom".to_string(), "42".to_string())].into_iter().collect(),
            ..Default::default()
        };
        let req = chat_request(&llm_client().unwrap(), &cfg, &[], 64).unwrap().build().unwrap();
        assert_eq!(req.headers().get("x-custom").unwrap(), "42");
    }

    #[test]
    fn message_thinks_detects_reasoning_fields() {
        assert!(!message_thinks(&json!({"content": "4"})));
        assert!(message_thinks(&json!({"content": "", "reasoning_content": "想…"})));
        assert!(message_thinks(&json!({"content": "x", "reasoning": {"foo": 1}})));
        assert!(message_thinks(&json!({"content": ""})));
        assert!(message_thinks(&json!({})));
    }

    /// Table translation: Q is a {"table": [[...]]} grid payload; a table with no parsable shape
    /// is skipped entirely (no tokens, no cover; the page is still marked done).
    #[test]
    fn collect_requests_packs_tables_and_skips_unparsable() {
        // type whitelist: table + text (unlisted types are not sent even with non-empty content)
        let cfg = LlmConfig { translate_types: vec!["table".into(), "text".into()], ..cfg(true) };
        let p = page(
            1,
            vec![
                blk("table", "<fcel>参 数<fcel>冬 季<nl><fcel>温度<fcel>18<nl>", None),
                blk("table", "没有表格标记", None),
                blk("text", "a", None),
            ],
            false,
        );
        let reqs = collect_requests(&cfg, &p, false);
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].q, json!({"table": [["参 数", "冬 季"], ["温度", "18"]]}));
        assert_eq!(reqs[0].table.as_ref().map(|g| g.cols), Some(2));
        assert_eq!(reqs[1].q, json!("a"));
        assert!(reqs[1].table.is_none());
    }

    /// Table result shape validation: a mismatch fails the whole page (strike budget backs off);
    /// A=null means the table is already in the target language → keep null.
    #[test]
    fn validate_result_checks_table_shape() {
        let grid = crate::table::parse_markup("<fcel>a<fcel>b<nl>").unwrap();
        let reqs = vec![RequestItem {
            index: "0".into(),
            q: json!({"table": [["a", "b"]]}),
            block_pos: 0,
            table: Some(grid),
        }];
        let ok = Reply { result: vec![("0".into(), Some("[[\"x\",\"y\"]]".into()))], ..Default::default() };
        assert_eq!(validate_result(&reqs, &ok).unwrap(), vec![Some("[[\"x\",\"y\"]]".to_string())]);
        let same = Reply { result: vec![("0".into(), None)], ..Default::default() };
        assert_eq!(validate_result(&reqs, &same).unwrap(), vec![None]);
        let bad = Reply { result: vec![("0".into(), Some("[[\"x\"]]".into()))], ..Default::default() };
        assert!(validate_result(&reqs, &bad).unwrap_err().contains("table result index 0 invalid"));
        let junk = Reply { result: vec![("0".into(), Some("x|y".into()))], ..Default::default() };
        assert!(validate_result(&reqs, &junk).unwrap_err().contains("is not JSON"));
    }

    /// Table persistence: the translation is written as a 2-D matrix JSON plus the grid in the bound JSON.
    #[test]
    fn apply_result_persists_table_grid() {
        let mut d = doc(vec![page(1, vec![blk("table", "<fcel>a<nl>", None)], false)]);
        let reqs = vec![RequestItem {
            index: "0".into(),
            q: json!({"table": [["a"]]}),
            block_pos: 0,
            table: crate::table::parse_markup("<fcel>a<nl>"),
        }];
        apply_result(&mut d.pages[0], &reqs, &[Some("[[\"A\"]]".into())]);
        let b = &d.pages[0].blocks[0];
        assert_eq!(b.translation.as_deref(), Some("[[\"A\"]]"));
        assert_eq!(b.grid.as_ref().map(|g| g.cols), Some(1));
    }

    /// Bypass tables store the source matrix + grid (not the markup); unparsable ones stay null.
    /// Translated-page backfill: only untranslated tables with a grid are sent, and same-language
    /// empty-translation blocks are never resent (else infinite re-translation).
    #[test]
    fn build_task_backfills_tables_on_translated_pages() {
        let table_cfg = LlmConfig { translate_types: vec!["table".into(), "text".into()], ..cfg(true) };
        let with_table = page(
            1,
            vec![
                blk("table", "<fcel>a<fcel>b<nl>", None),
                blk("text", "same-language null", None),
            ],
            true,
        );
        let t = build_task(&table_cfg, &doc(vec![with_table]), 1).expect("table backfill is a task");
        assert_eq!(t.requests.len(), 1, "only tables are collected on a translated page");
        assert!(t.requests[0].table.is_some());
        // no parsable grid → no backfill target → the whole page is skipped
        let unparsable = page(1, vec![blk("table", "没有表格标记", None)], true);
        assert!(build_task(&table_cfg, &doc(vec![unparsable]), 1).is_none());
        // table already translated → nothing to do
        let done = page(1, vec![blk("table", "<fcel>a<nl>", Some("[[\"A\"]]"))], true);
        assert!(build_task(&table_cfg, &doc(vec![done]), 1).is_none());
        // table not in translate types → nothing to do
        let no_table = LlmConfig { translate_types: vec!["text".into()], ..cfg(true) };
        let pending = page(1, vec![blk("table", "<fcel>a<nl>", None)], true);
        assert!(build_task(&no_table, &doc(vec![pending]), 1).is_none());
    }

    /// Table A=null (already target language) → store the source matrix as translation (terminal).
    #[test]
    fn apply_result_writes_source_matrix_for_null_tables() {
        let mut d = doc(vec![page(1, vec![blk("table", "<fcel>甲<fcel>乙<nl>", None)], false)]);
        let grid = crate::table::parse_markup("<fcel>甲<fcel>乙<nl>").unwrap();
        let reqs = vec![RequestItem {
            index: "0".into(),
            q: json!({"table": [["甲", "乙"]]}),
            block_pos: 0,
            table: Some(grid),
        }];
        apply_result(&mut d.pages[0], &reqs, &[None]);
        let b = &d.pages[0].blocks[0];
        assert_eq!(b.translation.as_deref(), Some("{\"table\":[[\"甲\",\"乙\"]]}"));
        assert_eq!(b.grid.as_ref().map(|g| g.cols), Some(2), "网格顺手落盘");
    }

    #[test]
    fn apply_bypass_writes_table_matrix_and_grid() {
        let cfg = LlmConfig { translate_types: vec!["table".into()], ..cfg(true) };
        let mut d = doc(vec![page(1, vec![blk("table", "<fcel>甲<fcel>乙<nl><fcel>丙<fcel>丁<nl>", None)], false)]);
        assert!(apply_bypass(&mut d, 1, &cfg));
        let b = &d.pages[0].blocks[0];
        assert_eq!(b.translation.as_deref(), Some("{\"table\":[[\"甲\",\"乙\"],[\"丙\",\"丁\"]]}"));
        assert_eq!(b.grid.as_ref().map(|g| g.cols), Some(2));
        let mut d2 = doc(vec![page(1, vec![blk("table", "没有表格标记", None)], false)]);
        assert!(apply_bypass(&mut d2, 1, &cfg));
        assert_eq!(d2.pages[0].blocks[0].translation, None);
        assert!(d2.pages[0].translated);
    }

    #[test]
    fn collect_requests_filters_and_maps_positions() {
        let p = page(
            1,
            vec![
                blk("text", "a", None),
                blk("table", "t", None),
                blk("text", "  ", None),
                blk("text", "b", Some("已译")),
                blk("content", "c", None),
            ],
            false,
        );
        let reqs = collect_requests(&cfg(true), &p, false);
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].index, "0");
        assert_eq!(reqs[0].q, "a");
        assert_eq!(reqs[0].block_pos, 0);
        assert_eq!(reqs[1].index, "1");
        assert_eq!(reqs[1].block_pos, 4);
    }

    /// Bypass path: source text becomes the translation and the page is marked done;
    /// non-translatable types stay null, already-translated pages untouched.
    #[test]
    fn apply_bypass_copies_content_and_marks_done() {
        let mut d = doc(vec![page(
            1,
            vec![blk("text", "原文", None), blk("table", "表格", None), blk("text", "已译", Some("旧"))],
            false,
        )]);
        assert!(apply_bypass(&mut d, 1, &cfg(true)));
        let p = &d.pages[0];
        assert_eq!(p.blocks[0].translation.as_deref(), Some("原文"));
        assert_eq!(p.blocks[1].translation, None, "table is not a translate type");
        assert_eq!(p.blocks[2].translation.as_deref(), Some("旧"), "existing translation is kept");
        assert!(p.translated);
        // idempotent: re-running on a translated page changes nothing
        assert!(!apply_bypass(&mut d, 1, &cfg(true)));
        // pages not OCR-finished are not processed
        let mut pending = doc(vec![PageInfo { finished: false, ..page(1, vec![blk("text", "x", None)], false) }]);
        assert!(!apply_bypass(&mut pending, 1, &cfg(true)));
    }

    /// Translate types are configurable: a non-empty set overrides the built-in default, empty falls back.
    #[test]
    fn translate_types_override_defaults() {
        let p = page(
            1,
            vec![blk("text", "a", None), blk("footer", "f", None), blk("content", "c", None)],
            false,
        );
        let custom = LlmConfig {
            translate_types: vec!["text".into(), "content".into()],
            ..cfg(true)
        };
        let reqs = collect_requests(&custom, &p, false);
        assert_eq!(reqs.len(), 2, "only the listed kinds are sent");
        assert_eq!(reqs[0].q, "a");
        assert_eq!(reqs[1].q, "c");
        let empty = LlmConfig { translate_types: Vec::new(), ..cfg(true) };
        assert_eq!(collect_requests(&empty, &p, false).len(), 3, "empty list falls back to defaults");
    }

    #[test]
    fn context_candidates_before_after_and_boundaries() {
        let d = doc(vec![
            page(1, vec![blk("text", "p1a", None), blk("text", "p1b", None)], true),
            page(
                2,
                vec![
                    blk("text", "t1", None),
                    blk("table", "x", None),
                    blk("text", "t2", None),
                    blk("text", "t3", None),
                    blk("text", "t4", None),
                    blk("text", "t5", None),
                ],
                true,
            ),
            page(3, vec![blk("text", "p3a", None)], false),
        ]);
        // before (neighbour 2): last ≤3 translatable blocks
        let b = context_candidates(&cfg(true), &d, 2, "before");
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].c, "t3");
        assert_eq!(b[2].c, "t5");
        // after (neighbour 2): first ≤3 translatable blocks
        let a = context_candidates(&cfg(true), &d, 2, "after");
        assert_eq!(a.len(), 3);
        assert_eq!(a[0].c, "t1");
        assert_eq!(a[2].c, "t3");
        // out-of-range page
        assert!(context_candidates(&cfg(true), &d, 99, "before").is_empty());
        assert_eq!(neighbor_index(1, "before"), None);
        assert_eq!(neighbor_index(3, "after"), Some(4));
        assert_eq!(neighbor_index(2, "before"), Some(1));
        assert_eq!(neighbor_index(2, "after"), Some(3));
    }

    #[test]
    fn parse_reply_handles_fences_null_and_index_types() {
        let raw = "```json\n{\"result\":[{\"index\":\"0\",\"A\":\"x\"},{\"index\":1,\"A\":null}],\"need_context\":false}\n```";
        let r = parse_reply(raw).unwrap();
        assert_eq!(r.result.len(), 2);
        assert_eq!(r.result[0], ("0".into(), Some("x".into())));
        assert_eq!(r.result[1], ("1".into(), None));
        assert!(r.need_context.is_none());

        // standard prompt shape: content key, no need_context
        let std = "{\"result\":[{\"index\":\"0\",\"content\":\"译\"}]}";
        let r = parse_reply(std).unwrap();
        assert_eq!(r.result[0], ("0".into(), Some("译".into())));
        assert!(r.need_context.is_none());

        let req = "{\"result\":[],\"need_context\":\"before\"}";
        let r = parse_reply(req).unwrap();
        assert_eq!(r.need_context.as_deref(), Some("before"));

        let d = "{\"result\":[{\"index\":\"0\",\"A\":\"y\"}],\"external_index\":2,\"external\":\"e\"}";
        let r = parse_reply(d).unwrap();
        assert_eq!(r.external_index.as_deref(), Some("2"));
        assert_eq!(r.external.as_deref(), Some("e"));

        assert!(parse_reply("not json").is_none());
    }

    #[test]
    fn validate_result_requires_full_match() {
        let reqs = vec![
            RequestItem { index: "0".into(), q: serde_json::json!("a"), block_pos: 0, table: None },
            RequestItem { index: "1".into(), q: serde_json::json!("b"), block_pos: 1, table: None },
        ];
        let ok = Reply {
            result: vec![("0".into(), Some("A".into())), ("1".into(), None)],
            ..Default::default()
        };
        let values = validate_result(&reqs, &ok).unwrap();
        assert_eq!(values, vec![Some("A".into()), None]);

        let short = Reply {
            result: vec![("0".into(), Some("A".into()))],
            ..Default::default()
        };
        assert!(validate_result(&reqs, &short).is_err());

        let missing = Reply {
            result: vec![("0".into(), Some("A".into())), ("9".into(), Some("B".into()))],
            ..Default::default()
        };
        assert!(validate_result(&reqs, &missing).is_err());
    }

    #[test]
    fn resolve_external_binds_only_valid_candidate() {
        let cands = vec![ContextItem { index: "0".into(), c: "half".into(), block_pos: 3 }];

        // valid index → (block pos, translation)
        let ok = Reply {
            external_index: Some("0".into()),
            external: Some("译文".into()),
            ..Default::default()
        };
        assert_eq!(resolve_external(&cands, &ok), Some((3, "译文".into())));

        // invalid index / missing external → unchanged
        let bad = Reply {
            external_index: Some("9".into()),
            external: Some("别的".into()),
            ..Default::default()
        };
        assert_eq!(resolve_external(&cands, &bad), None);
        let no_ext = Reply {
            external_index: Some("0".into()),
            ..Default::default()
        };
        assert_eq!(resolve_external(&cands, &no_ext), None);
    }

    #[test]
    fn build_task_skips_unfinished_translated_and_collects_context() {
        let d = doc(vec![
            page(1, vec![blk("text", "p1", None)], true),
            PageInfo {
                index: 2,
                finished: false,
                translated: false,
                blocks: vec![blk("text", "p2", None)],
            },
            page(3, vec![blk("text", "p3", None)], false),
        ]);
        // not OCR-finished → None; already translated → None
        assert!(build_task(&cfg(true), &d, 2).is_none());
        assert!(build_task(&cfg(true), &d, 1).is_none());
        // normal page: before p2 (1 text candidate), after p4 (no page → empty candidates)
        let t = build_task(&cfg(true), &d, 3).unwrap();
        assert_eq!(t.requests.len(), 1);
        assert_eq!(t.before.as_ref().map(|(n, c)| (*n, c.len())), Some((2, 1)));
        assert_eq!(t.after.as_ref().map(|(n, c)| (*n, c.len())), Some((4, 0)));
    }

    #[test]
    fn apply_done_writes_page_and_external() {
        let mut d = doc(vec![
            page(1, vec![blk("text", "half", None)], true),
            page(2, vec![blk("text", "cur", None)], false),
        ]);
        let done = PageDone {
            page_index: 2,
            requests: vec![RequestItem { index: "0".into(), q: serde_json::json!("cur"), block_pos: 0, table: None }],
            values: vec![Some("当前".into())],
            external: Some((1, 0, "半句译文".into())),
        };
        let mut touched = Vec::new();
        apply_done(&mut d, done, &mut touched);
        assert_eq!(d.pages[1].blocks[0].translation.as_deref(), Some("当前"));
        assert!(d.pages[1].translated);
        assert_eq!(d.pages[0].blocks[0].translation.as_deref(), Some("半句译文"));
        assert_eq!(touched, vec![2, 1]);
    }

    #[test]
    fn llm_config_usable_checks_three_fields() {
        let mut c = cfg(true);
        assert!(c.usable());
        c.model = "  ".into();
        assert!(!c.usable());
    }
}
