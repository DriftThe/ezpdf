//! LLM 翻译（阶段4 批4）：单页打包 + 智能上下文 agent loop + external 跨页绑定。
//!
//! 两套系统提示词（运行时从 system_prompt/ 读取，用户可改；EZPDF_SYSTEM_PROMPT_DIR 覆盖目录）：
//! - smart_context=true → `intelli_context.md`：协议 A–D（下文），result 项键为 `A`
//! - smart_context=false → `standard_translate.md`：无上下文协议，result 项键为 `content`
//! 解析层对两套都容错（A/content 均可、need_context 缺失即 None），非智能模式忽略任何上下文请求。
//!
//! 智能模式协议：
//! - 输入格式 A：`{requests: [{index, Q}]}`
//! - 正常输出 B：`{result: [{index, A}], need_context: false}`
//! - 截断请求：`{result: [], need_context: "before"|"after"}`
//! - 上下文 C：`{type: "before"|"after"|null, requests: [{index, C}]}`
//! - 联合输出 D：`{result, external_index, external}`
//!
//! 语义（用户拍板 2026-09-14）：agent loop 只允许一次上下文请求；`external` 写回
//! 相邻页命中候选块的 translation（该页再翻到时因 translation != null 自动跳过）；
//! 回退格式 B / 定位不到候选 → 不碰任何上下文框。
//!
//! 单页流程：收块（送翻类型、content 非空、translation == null）→ 格式 A →
//! 一轮或两轮（带 C）→ result 与 requests 严格一一对应（A=null 合法）→ 落盘；
//! 坏 JSON/缺 index 追加纠正消息重试（总轮数上限见 MAX_ROUNDS）。

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use ts_rs::TS;

use crate::parse::truncate;
use crate::{BindDoc, PageInfo};

/// invoke 传入的 LLM 配置（前端 settings store；dev 期由项目根 auth.cfg 临时填充）。
/// 后 8 个字段是 pi-ai 预设目录派生（用户 2026-09-15）：前端选供应商/模型时算好随
/// invoke 下发，Rust 只按数据施加，不内置目录（明细见 src/lib/piModels.ts）。
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct LlmConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub target_lang: String,
    /// 智能上下文翻译开关（默认开，见 LlmSection）
    #[serde(default)]
    pub smart_context: bool,
    /// 关思考请求参数策略："auto"（默认，按预设/端点推断）|
    /// "reasoning" | "enable_thinking" | "thinking_type" | "none"。
    /// 由设置页「验证」按钮探测后写入（用户 2026-09-14）；显式值优先于预设形态。
    #[serde(default)]
    pub thinking_off: String,
    /// 预设供应商 id（pi-ai 目录；"" = 自定义）
    #[serde(default)]
    pub provider: String,
    /// 预设协议（pi-ai api 字段；"" = 未知 → 按 OpenAI 兼容处理）。
    /// 非 "openai-completions" 的模型直接报错——当前客户端只说这一种协议。
    #[serde(default)]
    pub api: String,
    /// pi-ai thinkingFormat（openai | openrouter | deepseek | zai | qwen | qwen-chat-template）
    #[serde(default)]
    pub thinking_format: String,
    /// 预设派生的关思考施加方式（"" = 未知 → 退回端点 URL 规则）
    #[serde(default)]
    pub thinking_off_kind: String,
    /// reasoning_effort 取值（openrouter/openai 形态；None = 无法关闭思考）
    #[serde(default)]
    pub thinking_off_value: Option<String>,
    /// max tokens 字段名（"" = max_tokens；"max_completion_tokens" = 换名，pi-ai compat）
    #[serde(default)]
    pub max_tokens_field: String,
    /// 预设模型是否带思考模式（None = 未知）；false = 无需关思考参数
    #[serde(default)]
    pub model_reasoning: Option<bool>,
    /// 模型特有请求头（pi-ai 目录里少数模型有）
    #[serde(default)]
    pub extra_headers: std::collections::BTreeMap<String, String>,
}

/// 当前唯一支持的线上协议（pi-ai 的 api 字段取值）
const SUPPORTED_API: &str = "openai-completions";

impl LlmConfig {
    /// 三要素缺失即视为未配置：翻译整体跳过（OCR 不受影响）
    pub fn usable(&self) -> bool {
        !self.base_url.trim().is_empty()
            && !self.api_key.trim().is_empty()
            && !self.model.trim().is_empty()
    }
}

// ---- 翻译日志出口（设置页 LLM 面板底部实时展示；dev 终端同时可见）----

static LOG_SINK: OnceLock<AppHandle> = OnceLock::new();

/// lib.rs setup 时注入 AppHandle；单元测试/无 GUI 不注入 → 仅终端
pub fn init_log(app: &AppHandle) {
    let _ = LOG_SINK.set(app.clone());
}

/// 翻译链路日志（环形截断由前端负责；不落盘）
pub fn log(msg: impl AsRef<str>) {
    let msg = msg.as_ref();
    println!("[llm] {msg}");
    if let Some(app) = LOG_SINK.get() {
        let _ = app.emit("llm://log", msg.to_string());
    }
}

/// 送翻类型（与 src/lib/blocks.ts TRANSLATED_TYPES 同步；未知新标签默认不送翻）
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

/// 相邻页上下文候选上限（边界附近取 ≤N 个送翻块）
const CONTEXT_CANDIDATES: usize = 3;
/// 单页 LLM 调用总轮数上限（初始 + 上下文 + 纠正重试 ×2）
const MAX_ROUNDS: usize = 4;
const MAX_TOKENS: u32 = 8192;
const REQUEST_TIMEOUT_SECS: u64 = 180;

/// 提示词路径：EZPDF_SYSTEM_PROMPT_DIR 优先（环境变化只改这一处），
/// 否则 dev/源码态的仓库根 `system_prompt/`；按模式选用对应文件。
fn prompt_path(smart: bool) -> PathBuf {
    let file = if smart {
        "intelli_context.md"
    } else {
        "standard_translate.md"
    };
    if let Ok(dir) = std::env::var("EZPDF_SYSTEM_PROMPT_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir).join(file);
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../system_prompt")
        .join(file)
}

/// 读系统提示词 + 替换 {{target_language}}（文件按 smart_context 选择：智能/标准两套互不裁剪）
pub fn load_system_prompt(cfg: &LlmConfig) -> Result<String, String> {
    let path = prompt_path(cfg.smart_context);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取翻译提示词失败 {}: {e}", path.display()))?;
    let lang = if cfg.target_lang.trim().is_empty() {
        "Simplified Chinese"
    } else {
        cfg.target_lang.trim()
    };
    let text = text.replace("{{target_language}}", lang);
    // 兜底（用户 2026-09-14）：提示词被大改成没有 {{target_language}} 占位符时，
    // 仍把目标语言追加进系统提示——保证目标语言一定在提示词里
    Ok(if text.contains(lang) {
        text
    } else {
        format!("{text}\n\n目标语言：{lang}。所有译文必须使用该语言。\n")
    })
}

fn is_translatable(kind: &str, content: &str) -> bool {
    TRANSLATABLE_TYPES.contains(&kind) && !content.trim().is_empty()
}

/// 待翻请求：index 字符串（协议原样）+ 块在页内位置
#[derive(Clone)]
struct RequestItem {
    index: String,
    q: String,
    block_pos: usize,
}

/// 收集该页送翻块：送翻类型、content 非空、translation == null
/// （已由邻页 external 绑定的块自动跳过——用户拍板的"不再翻译该文本框"）
fn collect_requests(page: &PageInfo) -> Vec<RequestItem> {
    page.blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| is_translatable(&b.kind, &b.content) && b.translation.is_none())
        .enumerate()
        .map(|(i, (pos, b))| RequestItem {
            index: i.to_string(),
            q: b.content.clone(),
            block_pos: pos,
        })
        .collect()
}

/// 上下文候选：index 字符串 + C 文本 + 块位置
#[derive(Clone)]
struct ContextItem {
    index: String,
    c: String,
    block_pos: usize,
}

/// direction → 相邻页 index（1-based：before=上一页、after=下一页；越界 None）
fn neighbor_index(page_index: u32, direction: &str) -> Option<u32> {
    match direction {
        "before" => (page_index > 1).then(|| page_index - 1),
        "after" => Some(page_index + 1),
        _ => None,
    }
}

/// 邻页边界候选：before 取末尾 ≤N、after 取开头 ≤N 个送翻块（含已翻译块：
/// 只提供原文参考，外部绑定命中时可覆盖旧值）
fn context_candidates(doc: &BindDoc, neighbor: u32, direction: &str) -> Vec<ContextItem> {
    let Some(page) = doc.pages.iter().find(|p| p.index == neighbor) else {
        return Vec::new();
    };
    let mut picked: Vec<(usize, &crate::Block)> = page
        .blocks
        .iter()
        .enumerate()
        .filter(|(_, b)| is_translatable(&b.kind, &b.content))
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
            c: b.content.clone(),
            block_pos: pos,
        })
        .collect()
}

/// LLM 回复解析结果
#[derive(Debug, Default)]
struct Reply {
    result: Vec<(String, Option<String>)>,
    /// "before" | "after"（None = 未请求上下文）
    need_context: Option<String>,
    external: Option<String>,
    external_index: Option<String>,
}

/// 容错提取 JSON：剥 ``` 围栏，取首个 `{` 到末个 `}`（模型偶发加壳）
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

/// index 字段容错：字符串原样 / 数字转字符串；其他类型 None
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
            // 译文键兼容两套提示词：智能模式 `A` / 标准模式 `content`
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

/// result 与 requests 严格一一对应（长度 + 每个 index）；A=null 合法（同语种）
fn validate_result(
    requests: &[RequestItem],
    reply: &Reply,
) -> Result<Vec<Option<String>>, String> {
    if reply.result.len() != requests.len() {
        return Err(format!(
            "result 数量不匹配: 请求 {} 返回 {}",
            requests.len(),
            reply.result.len()
        ));
    }
    let mut out = Vec::with_capacity(requests.len());
    for req in requests {
        match reply.result.iter().find(|(i, _)| *i == req.index) {
            Some((_, a)) => out.push(a.clone()),
            None => return Err(format!("result 缺少 index {}", req.index)),
        }
    }
    Ok(out)
}

/// 译文写回（A=None 不改：保持 null → 渲染原文 content）
fn apply_result(page: &mut PageInfo, requests: &[RequestItem], values: &[Option<String>]) {
    for (req, val) in requests.iter().zip(values) {
        if let Some(v) = val {
            if let Some(block) = page.blocks.get_mut(req.block_pos) {
                block.translation = Some(v.clone());
            }
        }
    }
}

/// external 命中候选（external_index 必须来自本次 C）→ (块位置, 译文)；
/// 未命中/格式不全返回 None（不碰任何上下文框）
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
        .map_err(|e| format!("LLM HTTP 客户端创建失败: {e}"))
}

/// opencode zen 端点要求会话路由头（任意非空值即可）；其他 OpenAI 兼容端点
/// 忽略该头。仅在 base_url 命中 opencode.ai 时发送（见 PLAN-LLM.md §5）。
const OPENCODE_SESSION: &str = "ezpdf";

/// 关思考参数：显式策略 > 实测端点规则 > pi-ai 预设形态（用户 2026-09-15）。
/// - 显式策略来自设置页「验证」探测（用户 2026-09-14），仍然最优先；
/// - opencode zen / SiliconFlow 是实测过的硬规则（见 PLAN-LLM.md §5），不交给预设覆盖；
/// - 预设形态对应 pi-ai openai-completions provider 的 buildParams：deepseek 用
///   thinking.type、zai/qwen 用 enable_thinking、openrouter 用 reasoning.effort、
///   openai 仅在目录给了 off 值时才写 reasoning_effort；"none" = 不加参数。
fn apply_thinking_off(body: &mut Value, url: &str, cfg: &LlmConfig) {
    match cfg.thinking_off.trim() {
        "reasoning" => {
            body["reasoning"] = json!({"enabled": false});
            body["reasoning_effort"] = json!("none");
            return;
        }
        "enable_thinking" => {
            body["enable_thinking"] = json!(false);
            return;
        }
        "thinking_type" => {
            body["thinking"] = json!({"type": "disabled"});
            return;
        }
        "none" => return,
        _ => {}
    }
    // 预设标记为非思考模型：pi-ai 同样不写任何思考参数
    if cfg.model_reasoning == Some(false) {
        return;
    }
    // auto：先走实测端点规则
    if url.contains("opencode.ai") {
        // opencode zen（OpenRouter 系）
        body["reasoning"] = json!({"enabled": false});
        body["reasoning_effort"] = json!("none");
        return;
    }
    if url.contains("siliconflow") {
        // SiliconFlow（Qwen3.5 默认开思考）
        body["enable_thinking"] = json!(false);
        body["thinking"] = json!({"type": "disabled"});
        return;
    }
    // auto：pi-ai 预设形态
    match cfg.thinking_off_kind.trim() {
        "thinking_type" => body["thinking"] = json!({"type": "disabled"}),
        "enable_thinking" => body["enable_thinking"] = json!(false),
        "chat_template_kwargs" => {
            body["chat_template_kwargs"] = json!({"enable_thinking": false, "preserve_thinking": true});
        }
        "reasoning_effort" => {
            let value = cfg.thinking_off_value.clone().unwrap_or_else(|| "none".into());
            body["reasoning_effort"] = json!(value);
        }
        _ => {}
    }
}

/// OpenAI 兼容 /chat/completions 请求（协议校验 + max tokens 字段名 + 关思考策略 +
/// zen 路由头 + 预设模型特有头），供对话与验证共用
fn chat_request(
    client: &reqwest::Client,
    cfg: &LlmConfig,
    messages: &[Value],
    max_tokens: u32,
) -> Result<reqwest::RequestBuilder, String> {
    // 预设协议不匹配就直接失败（用户 2026-09-15）：与其发出去被 400/乱答，不如说清原因
    if !cfg.api.trim().is_empty() && cfg.api.trim() != SUPPORTED_API {
        return Err(format!(
            "模型 {} 使用 {} 协议，当前仅支持 OpenAI 兼容端点（{SUPPORTED_API}）",
            cfg.model,
            cfg.api.trim()
        ));
    }
    let url = format!(
        "{}/chat/completions",
        cfg.base_url.trim().trim_end_matches('/')
    );
    // max tokens 字段名：pi-ai compat 判定（标准 OpenAI 系用 max_completion_tokens）
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
    apply_thinking_off(&mut body, &url, cfg);
    let mut req = client.post(&url).bearer_auth(&cfg.api_key);
    if url.contains("opencode.ai") {
        req = req.header("x-opencode-session", OPENCODE_SESSION);
    }
    for (key, value) in &cfg.extra_headers {
        req = req.header(key, value);
    }
    Ok(req.json(&body))
}

/// 发一次对话请求，返回 choices[0].message 原始对象（取 content 与思考检测共用）
async fn chat_raw(
    client: &reqwest::Client,
    cfg: &LlmConfig,
    messages: &[Value],
    max_tokens: u32,
) -> Result<Value, String> {
    let resp = chat_request(client, cfg, messages, max_tokens)?
        .send()
        .await
        .map_err(|e| format!("LLM 请求失败: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("LLM 响应读取失败: {e}"))?;
    if !status.is_success() {
        return Err(format!("LLM 返回 {status}: {}", truncate(&text, 300)));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("LLM 响应非 JSON: {e}"))?;
    let msg = &v["choices"][0]["message"];
    if msg.is_object() {
        Ok(msg.clone())
    } else {
        Err(format!(
            "LLM 响应缺少 choices[0].message: {}",
            truncate(&text, 200)
        ))
    }
}

/// OpenAI 兼容 /chat/completions（非流式）；返回首个 choice 的 content
async fn chat(client: &reqwest::Client, cfg: &LlmConfig, messages: &[Value]) -> Result<String, String> {
    let msg = chat_raw(client, cfg, messages, MAX_TOKENS).await?;
    msg["content"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| format!("LLM 响应缺少 choices[0].message.content: {}", truncate(&msg.to_string(), 200)))
}

// ---- 设置页「验证」按钮（用户 2026-09-14）：连通性检查 + 关思考策略探测 ----

/// 探测请求：短问短答，max_tokens 故意小——思考模型会先被思考吃满导致 content 为空
const PROBE_PROMPT: &str = "不要思考，直接回答：2+2 等于几？只输出数字。";
const PROBE_MAX_TOKENS: u32 = 64;

/// 验证报告（设置页 LLM 面板展示 + 回写 thinkingOff 策略）
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct LlmVerifyReport {
    /// 生效策略；"none" = 连通但尝试后仍无法关闭思考（前端 toast 警告）
    pub strategy: String,
    /// 预设标记为非思考模型（此时 strategy 的 "none" 不是问题，前端不告警）
    pub preset_no_thinking: bool,
    /// 展示文案（含耗时与策略说明）
    pub message: String,
}

/// message 是否仍带思考：reasoning_content/reasoning 字段非空，或 content 被吃空
fn message_thinks(msg: &Value) -> bool {
    for key in ["reasoning_content", "reasoning"] {
        match msg.get(key) {
            Some(Value::String(s)) => {
                if !s.trim().is_empty() {
                    return true;
                }
            }
            Some(Value::Null) | None => {}
            Some(_) => return true, // 对象/数组形态的 reasoning（OpenRouter）
        }
    }
    msg.get("content")
        .and_then(|c| c.as_str())
        .map(|c| c.trim().is_empty())
        .unwrap_or(true)
}

/// 验证 LLM：先按 auto 发探测（失败即连通性错误，原样返回），仍在思考则逐个试显式策略。
/// 预设模型若标记为非思考（pi-ai 目录 reasoning=false）则跳过策略搜索。
pub async fn verify_llm(cfg: &LlmConfig) -> Result<LlmVerifyReport, String> {
    if !cfg.usable() {
        return Err("请先填写 Base URL / API Key / 模型".into());
    }
    let client = llm_client()?;
    let messages = vec![json!({"role": "user", "content": PROBE_PROMPT})];
    let started = std::time::Instant::now();

    let mut probe = cfg.clone();
    probe.thinking_off = "auto".into();
    let first = chat_raw(&client, &probe, &messages, PROBE_MAX_TOKENS).await?; // 连通性失败 → 直接上报
    let preset_no_thinking = cfg.model_reasoning == Some(false);
    if preset_no_thinking {
        return Ok(LlmVerifyReport {
            strategy: "none".into(),
            preset_no_thinking,
            message: format!(
                "连接正常（{}ms）；预设标记为非思考模型，无需关思考参数",
                started.elapsed().as_millis()
            ),
        });
    }
    let mut strategy = if !message_thinks(&first) { "auto" } else { "" };
    if strategy.is_empty() {
        for cand in ["reasoning", "enable_thinking", "thinking_type"] {
            let mut p = cfg.clone();
            p.thinking_off = cand.into();
            match chat_raw(&client, &p, &messages, PROBE_MAX_TOKENS).await {
                Ok(msg) if !message_thinks(&msg) => {
                    strategy = cand;
                    break;
                }
                _ => continue, // 端点拒绝该参数或仍未关思考 → 试下一个
            }
        }
    }
    let ms = started.elapsed().as_millis();
    Ok(if strategy.is_empty() {
        LlmVerifyReport {
            strategy: "none".into(),
            preset_no_thinking,
            message: format!("连接正常（{ms}ms），但尝试后无法关闭思考模式"),
        }
    } else {
        LlmVerifyReport {
            strategy: strategy.to_string(),
            preset_no_thinking,
            message: format!("连接正常（{ms}ms），思考关闭策略：{strategy}"),
        }
    })
}

/// 拉取 OpenAI 兼容 /models 列表（模型输入框自动补全；zen 端点补会话头）
pub async fn fetch_models(base_url: &str, api_key: &str) -> Result<Vec<String>, String> {
    if base_url.trim().is_empty() || api_key.trim().is_empty() {
        return Err("请先填写 Base URL / API Key".into());
    }
    let client = llm_client()?;
    let url = format!("{}/models", base_url.trim().trim_end_matches('/'));
    let mut req = client.get(&url).bearer_auth(api_key);
    if url.contains("opencode.ai") {
        req = req.header("x-opencode-session", OPENCODE_SESSION);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("获取模型列表失败: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("获取模型列表失败: {e}"))?;
    if !status.is_success() {
        return Err(format!("获取模型列表返回 {status}: {}", truncate(&text, 200)));
    }
    let v: Value = serde_json::from_str(&text).map_err(|e| format!("模型列表非 JSON: {e}"))?;
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
    "已提供上下文，请立即按格式 B 或 D 输出最终翻译，不要再请求上下文。";

/// 单页翻译快照：并发任务只读自己的快照，不触碰 &mut BindDoc（避免并发读写文档）；
/// 上下文候选在快照时一次取好（before/after 各一份），模型请求哪个方向用哪个
pub struct PageTask {
    page_index: u32,
    requests: Vec<RequestItem>,
    before: Option<(u32, Vec<ContextItem>)>,
    after: Option<(u32, Vec<ContextItem>)>,
}

/// 单页翻译结果：由调度方在锁内落到 BindDoc
pub struct PageDone {
    page_index: u32,
    requests: Vec<RequestItem>,
    values: Vec<Option<String>>,
    /// (邻页 index, 邻页块位置, 译文)：external 跨页绑定
    external: Option<(u32, usize, String)>,
}

/// 文档快照 → 单页任务；页不存在/未 OCR 完成/已翻译 → None（跳过）
pub fn build_task(doc: &BindDoc, page_index: u32) -> Option<PageTask> {
    let page = doc.pages.iter().find(|p| p.index == page_index)?;
    if !page.finished || page.translated {
        return None;
    }
    Some(PageTask {
        page_index,
        requests: collect_requests(page),
        before: neighbor_index(page_index, "before")
            .map(|n| (n, context_candidates(doc, n, "before"))),
        after: neighbor_index(page_index, "after")
            .map(|n| (n, context_candidates(doc, n, "after"))),
    })
}

/// 结果落盘（锁内调用）：写页译文 + translated 标记 + external 邻页块；
/// touched 收集实际变更页（含被 external 补写的邻页，供 updatedPages 回传）
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

/// 组上下文应答（协议格式 C，仅智能模式路径调用）：请求方向有邻页才提供候选，
/// 否则回 `type: null`；返回 (发给模型的 C 消息, 待回写 external 的邻页上下文)。
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
            "p{page_index} 第{}轮 {ms}ms 请求 {dir} 上下文 → p{n} 无候选，回 null",
            round + 1
        )),
        Some(n) => log(format!(
            "p{page_index} 第{}轮 {ms}ms 请求 {dir} 上下文 → p{n} 候选 {} 块",
            round + 1,
            candidates.len()
        )),
        None => log(format!(
            "p{page_index} 第{}轮 {ms}ms 请求 {dir} 上下文 → 无候选，回 null",
            round + 1
        )),
    }
    (c, neighbor.map(|n| (n, candidates)))
}

/// 翻译单页（无文档依赖）：成功返回结果（含 external 绑定）；失败返回 Err。
/// requests 为空（空文本/已 external 绑定）→ 直接返回空结果（调用方标记 translated）
pub async fn translate_task(
    cfg: &LlmConfig,
    prompt: &str,
    client: &reqwest::Client,
    task: PageTask,
) -> Result<PageDone, String> {
    let PageTask { page_index, requests, before, after } = task;
    if requests.is_empty() {
        log(format!("p{page_index} 无可翻块（空文本/已绑定）→ 标记完成"));
        return Ok(PageDone {
            page_index,
            requests,
            values: Vec::new(),
            external: None,
        });
    }
    log(format!("p{page_index} 送翻 {} 块", requests.len()));

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

    let mut context_answered = false; // 已发过 C（或 null 应答）
    // 已发出的上下文（邻页 index + 候选）：最终输出回执后用于 external 落盘
    let mut context: Option<(u32, Vec<ContextItem>)> = None;

    for _round in 0..MAX_ROUNDS {
        let started = std::time::Instant::now();
        let raw = chat(client, cfg, &messages).await?;
        let ms = started.elapsed().as_millis();
        let reply = match parse_reply(&raw) {
            Some(r) => r,
            None => {
                log(format!(
                    "p{page_index} 第{}轮 {ms}ms 输出无法解析 → 纠正重试",
                    _round + 1
                ));
                messages.push(json!({"role": "assistant", "content": raw}));
                messages.push(json!({"role": "user", "content": CORRECTION}));
                continue;
            }
        };

        // 上下文请求：仅智能模式处理（标准模式无此协议，忽略后走 result 校验）
        if cfg.smart_context {
            if let Some(dir) = reply.need_context.clone() {
                if context_answered {
                    log(format!("p{page_index} 第{}轮 {ms}ms 重复请求上下文 → 逼最终输出", _round + 1));
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

        // 最终输出：校验 → 返回结果
        let values = match validate_result(&requests, &reply) {
            Ok(v) => v,
            Err(e) => {
                log(format!(
                    "p{page_index} 第{}轮 {ms}ms result 校验失败（{e}）→ 纠正重试",
                    _round + 1
                ));
                messages.push(json!({"role": "assistant", "content": raw}));
                messages.push(json!({"role": "user", "content": format!("{CORRECTION}（问题：{e}）")}));
                continue;
            }
        };
        let nulls = values.iter().filter(|v| v.is_none()).count();
        log(format!(
            "p{page_index} 完成：{} 块译文（null {nulls}）",
            values.len() - nulls
        ));
        let external = match context.as_ref() {
            Some((neighbor, candidates)) => match resolve_external(candidates, &reply) {
                Some((pos, text)) => {
                    log(format!(
                        "p{page_index} external 回写 p{neighbor}（候选 index {:?}）",
                        reply.external_index
                    ));
                    Some((*neighbor, pos, text))
                }
                None => {
                    if reply.external_index.is_some() {
                        log(format!(
                            "p{page_index} external_index {:?} 未命中候选 → 忽略",
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
    Err(format!("页 {page_index} 翻译失败：LLM 输出重试耗尽"))
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

    /// 关思考施加的输入：显式策略 + pi-ai 预设形态
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
    fn prompt_files_load_by_mode_and_template_lang() {
        let smart = cfg(true);
        let text = load_system_prompt(&smart).unwrap();
        assert!(text.contains("Simplified Chinese"));
        assert!(text.contains("need_context")); // 智能协议在文中
        let plain = cfg(false);
        let text = load_system_prompt(&plain).unwrap();
        assert!(text.contains("Simplified Chinese"));
        assert!(!text.contains("need_context")); // 标准提示词无上下文协议
        assert!(text.contains("\"content\""));
        // 目标语言占位符被删时兜底追加
        let mut custom = cfg(false);
        custom.target_lang = "Klingon".into();
        let text = load_system_prompt(&custom).unwrap();
        assert!(text.contains("Klingon"));
    }

    #[test]
    fn thinking_off_strategies_shape_body() {
        let explicit = |s: &str| think_cfg(s, "", None, None);
        let mut body = json!({});
        apply_thinking_off(&mut body, "https://x/v1", &explicit("reasoning"));
        assert_eq!(body["reasoning"]["enabled"], json!(false));
        assert_eq!(body["reasoning_effort"], json!("none"));

        let mut body = json!({});
        apply_thinking_off(&mut body, "https://x/v1", &explicit("enable_thinking"));
        assert_eq!(body["enable_thinking"], json!(false));

        let mut body = json!({});
        apply_thinking_off(&mut body, "https://x/v1", &explicit("thinking_type"));
        assert_eq!(body["thinking"]["type"], json!("disabled"));

        // auto 矩阵（实测过的端点规则）：zen → reasoning；siliconflow → enable_thinking+thinking
        let auto = |kind: &str, value: Option<&str>, reasoning: Option<bool>| {
            think_cfg("auto", kind, value, reasoning)
        };
        let mut body = json!({});
        apply_thinking_off(&mut body, "https://zen.opencode.ai/v1", &auto("", None, None));
        assert_eq!(body["reasoning"]["enabled"], json!(false));
        let mut body = json!({});
        apply_thinking_off(&mut body, "https://api.siliconflow.cn/v1", &auto("", None, None));
        assert_eq!(body["enable_thinking"], json!(false));
        assert_eq!(body["thinking"]["type"], json!("disabled"));

        // 未知端点不加参数；显式 none 同样不加
        let mut body = json!({});
        apply_thinking_off(&mut body, "https://api.openai.com/v1", &auto("", None, None));
        assert!(body.as_object().unwrap().is_empty());
        let mut body = json!({});
        apply_thinking_off(&mut body, "https://api.openai.com/v1", &explicit("none"));
        assert!(body.as_object().unwrap().is_empty());

        // pi-ai 预设形态（用户 2026-09-15）：deepseek / zai / qwen / openrouter / openai
        let mut body = json!({});
        apply_thinking_off(&mut body, "https://x/v1", &auto("thinking_type", None, Some(true)));
        assert_eq!(body["thinking"]["type"], json!("disabled"));
        let mut body = json!({});
        apply_thinking_off(&mut body, "https://x/v1", &auto("enable_thinking", None, Some(true)));
        assert_eq!(body["enable_thinking"], json!(false));
        let mut body = json!({});
        apply_thinking_off(&mut body, "https://x/v1", &auto("chat_template_kwargs", None, Some(true)));
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], json!(false));
        let mut body = json!({});
        apply_thinking_off(&mut body, "https://x/v1", &auto("reasoning_effort", None, Some(true)));
        assert_eq!(body["reasoning_effort"], json!("none")); // openrouter 缺省 off = "none"
        let mut body = json!({});
        apply_thinking_off(&mut body, "https://x/v1", &auto("reasoning_effort", Some("low"), Some(true)));
        assert_eq!(body["reasoning_effort"], json!("low"));
        // 预设 openai 形态且目录没给 off 值 → 不加参数（与 pi-ai 一致）
        let mut body = json!({});
        apply_thinking_off(&mut body, "https://x/v1", &auto("none", None, Some(true)));
        assert!(body.as_object().unwrap().is_empty());
        // 预设标记为非思考模型 → 不加参数（即使形态表有值）
        let mut body = json!({});
        apply_thinking_off(&mut body, "https://x/v1", &auto("enable_thinking", None, Some(false)));
        assert!(body.as_object().unwrap().is_empty());

        // 协议不支持：请求直接失败（不进 body 构造）
        let cfg = LlmConfig {
            base_url: "https://x/v1".into(),
            api_key: "k".into(),
            model: "claude-x".into(),
            api: "anthropic-messages".into(),
            ..Default::default()
        };
        let err = chat_request(&llm_client().unwrap(), &cfg, &[], 64).unwrap_err();
        assert!(err.contains("anthropic-messages"), "{err}");
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
        let reqs = collect_requests(&p);
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].index, "0");
        assert_eq!(reqs[0].q, "a");
        assert_eq!(reqs[0].block_pos, 0);
        assert_eq!(reqs[1].index, "1");
        assert_eq!(reqs[1].block_pos, 4);
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
        // before（邻页 2）：取末尾 ≤3 个送翻块
        let b = context_candidates(&d, 2, "before");
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].c, "t3");
        assert_eq!(b[2].c, "t5");
        // after（邻页 2）：取开头 ≤3 个送翻块
        let a = context_candidates(&d, 2, "after");
        assert_eq!(a.len(), 3);
        assert_eq!(a[0].c, "t1");
        assert_eq!(a[2].c, "t3");
        // 越界页
        assert!(context_candidates(&d, 99, "before").is_empty());
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

        // 标准提示词形态：content 键、无 need_context
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
            RequestItem { index: "0".into(), q: "a".into(), block_pos: 0 },
            RequestItem { index: "1".into(), q: "b".into(), block_pos: 1 },
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

        // 有效 index → 返回 (块位置, 译文)
        let ok = Reply {
            external_index: Some("0".into()),
            external: Some("译文".into()),
            ..Default::default()
        };
        assert_eq!(resolve_external(&cands, &ok), Some((3, "译文".into())));

        // 无效 index / 缺 external → 不改
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
        // 未 OCR 完成 → None；已翻译 → None
        assert!(build_task(&d, 2).is_none());
        assert!(build_task(&d, 1).is_none());
        // 正常页：before 取 p2（虽有块但未送翻候选？p2 有 text 块 → 候选 1），after 取 p4（无页 → 空候选）
        let t = build_task(&d, 3).unwrap();
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
            requests: vec![RequestItem { index: "0".into(), q: "cur".into(), block_pos: 0 }],
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
