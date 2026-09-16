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
/// api/thinkingOffKind/thinkingOffValue/maxTokensField/modelReasoning/extraHeaders 是
/// pi-ai 预设目录派生（用户 2026-09-15）：前端选供应商/模型时算好随 invoke 下发，
/// Rust 只按数据施加，不内置目录（明细见 src/lib/piModels.ts）。
#[derive(Debug, Clone, Deserialize)]
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
    /// 预设协议（pi-ai api 字段）："openai-completions"（缺省）| "anthropic-messages" |
    /// "openai-responses"；"" = 旧配置/自定义未指定 → 按 OpenAI Chat Completions 处理。
    /// 其余取值直接报错（见 Protocol::from_api），前端也不会列出这类预设。
    #[serde(default)]
    pub api: String,
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
    /// 参与翻译的块类型（用户 2026-09-15 可配，见 设置→常规）：
    /// 空 Vec = 内置默认（TRANSLATABLE_TYPES）；非空则只翻列出的类型。
    /// 只影响尚未翻译的页面（已翻页面在 JSON 里已有译文）。
    #[serde(default)]
    pub translate_types: Vec<String>,
    /// 是否启用翻译（用户 2026-09-15，设置→LLM 首项）：false = 不请求 LLM，
    /// 把送翻块的 content 直接当作 translation 落盘并标记页面完成（见 apply_bypass）。
    /// 老配置没有这个键 → true（保持原行为）。
    #[serde(default = "translate_enabled_default")]
    pub translate_enabled: bool,
}

/// 缺省 true：只有显式关掉翻译才走 bypass 路径
fn translate_enabled_default() -> bool {
    true
}

/// 手写 Default（不能用 derive）：derive 会让 translate_enabled 变 false，
/// 与 serde 的缺省 true 相反——测试里的 `..Default::default()` 会悄悄走 bypass 路径。
/// 与 [`translate_enabled_default`] 保持一致。
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

/// opencode zen 端点：除 bearer 外还要带会话路由头（PLAN-LLM.md §5）
fn is_opencode(url: &str) -> bool {
    url.contains("opencode.ai")
}

// ---- 线上协议（用户 2026-09-16：Messages / Responses 加入支持）----------------------------
//
// 三种协议的差异只在「请求体形状 + 认证头 + 响应取字段」三处，翻译链本身（送翻块打包、
// 上下文协议、纠正重试、关思考策略）完全共用——所以这里只做线协议适配，不做流程分叉。

/// OpenAI 兼容 Chat Completions（缺省；`api` 为空也走这里）
const API_CHAT: &str = "openai-completions";
/// Anthropic Messages（Claude 原生；`POST {base}/v1/messages`）
const API_MESSAGES: &str = "anthropic-messages";
/// OpenAI Responses（`POST {base}/responses`）
const API_RESPONSES: &str = "openai-responses";
/// Anthropic 的版本头（@anthropic-ai/sdk 的默认值，必填）
const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Protocol {
    Chat,
    Messages,
    Responses,
}

impl Protocol {
    /// `api` 字段 → 协议；暂不支持的值直接报错（与其发出去被 400/乱答，不如说清原因）
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

    /// Anthropic 的 baseURL 约定不带版本段（SDK 内部补 `/v1`）——这里同时容忍用户
    /// 直接粘了 `.../v1` 甚至完整 `.../v1/messages` 的写法（自定义端点最容易踩）
    fn anthropic_base(base_url: &str) -> &str {
        let base = base_url.trim().trim_end_matches('/');
        let base = base.strip_suffix("/messages").unwrap_or(base);
        base.strip_suffix("/v1").unwrap_or(base)
    }

    /// 对话请求地址：chat → `/chat/completions`、messages → `/v1/messages`、
    /// responses → `/responses`（后两者的 base 按各自 SDK 约定自带/不带版本段）
    fn chat_url(self, base_url: &str) -> String {
        let base = base_url.trim().trim_end_matches('/');
        match self {
            Protocol::Chat => format!("{base}/chat/completions"),
            Protocol::Messages => format!("{}/v1/messages", Self::anthropic_base(base_url)),
            Protocol::Responses => format!("{base}/responses"),
        }
    }

    /// 模型列表地址（Anthropic 是 `/v1/models`，另两家的 base 自带版本段）
    fn models_url(self, base_url: &str) -> String {
        let base = base_url.trim().trim_end_matches('/');
        match self {
            Protocol::Messages => format!("{}/v1/models", Self::anthropic_base(base_url)),
            _ => format!("{base}/models"),
        }
    }
}

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

/// 送翻类型默认值（与 src/lib/blocks.ts BLOCK_TYPE_OPTIONS 同步；未知新标签默认不送翻）。
/// 用户在 设置→常规 改过的集合随 invoke 下发（LlmConfig.translate_types），非空时覆盖本默认。
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

/// 提示词目录：setup 钩子里解析一次（EZPDF_SYSTEM_PROMPT_DIR 覆盖 > 生产资源目录 >
/// dev 仓库根）。不能只在 prompt_path 里用编译期 CARGO_MANIFEST_DIR——那是构建机的路径，
/// 而打包安装后 system_prompt/ 是安装包里的资源（tauri.conf 的 bundle.resources），
/// 只有 resource_dir() 能找到（生产漏解析 = 装完翻译取不到提示词）。
static PROMPT_DIR: OnceLock<PathBuf> = OnceLock::new();

/// setup 钩子调用：解析提示词目录并存入全局（与 pyenv 的 PyPaths 同一范式）
pub fn init_prompt_dir(app: &AppHandle) {
    let dir = resolve_prompt_dir(app);
    println!("[ezpdf] system_prompt dir = {}", dir.display());
    let _ = PROMPT_DIR.set(dir);
}

/// 纯函数便于单测：优先级 = env 覆盖（非空白）> 资源目录（含提示词文件）> dev 仓库根
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
        let _ = app; // dev 分支用不到 AppHandle
        pick_prompt_dir(env.as_deref(), None, &dev_prompt_dir())
    }
    #[cfg(not(dev))]
    {
        // 与 pyenv 的 server_root 同口径：bundle.resources 的 target 相对资源根，
        // 再兼容一层 resources/ 子目录（自定义打包布局）
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

/// 提示词路径：按模式选用对应文件
fn prompt_path(smart: bool) -> PathBuf {
    let file = if smart {
        "intelli_context.md"
    } else {
        "standard_translate.md"
    };
    let dir = match PROMPT_DIR.get() {
        Some(dir) => dir.clone(),
        // 未经 setup（单测）：退回 env 覆盖 / dev 仓库根
        None => pick_prompt_dir(std::env::var("EZPDF_SYSTEM_PROMPT_DIR").ok().as_deref(), None, &dev_prompt_dir()),
    };
    dir.join(file)
}

/// 读系统提示词 + 替换 {{target_language}}（文件按 smart_context 选择：智能/标准两套互不裁剪）
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
    // 兜底（用户 2026-09-14）：提示词被大改成没有 {{target_language}} 占位符时，
    // 仍把目标语言追加进系统提示——保证目标语言一定在提示词里
    Ok(if text.contains(lang) {
        text
    } else {
        format!("{text}\n\n目标语言：{lang}。所有译文必须使用该语言。\n")
    })
}

/// 该块是否送翻：类型在允许集合内 + content 非空
fn is_translatable(cfg: &LlmConfig, kind: &str, content: &str) -> bool {
    let allowed = if cfg.translate_types.is_empty() {
        TRANSLATABLE_TYPES.contains(&kind)
    } else {
        cfg.translate_types.iter().any(|t| t == kind)
    };
    allowed && !content.trim().is_empty()
}

/// 待翻请求：index 字符串（协议原样）+ 块在页内位置 + 送翻内容。
/// q 是 JSON 值：常规块是字符串、表块是 `{"table": [[...]]}` 对象（用户 2026-09-16：
/// 表格打包成 JSON 结构送翻，见 table.rs）。
#[derive(Clone)]
struct RequestItem {
    index: String,
    q: serde_json::Value,
    block_pos: usize,
    /// 表块：本次请求用的网格（结果形状校验 + 落盘 grid 都用它）；非表块 None
    table: Option<crate::table::TableGrid>,
}

/// 表块网格：优先用绑定 JSON 里已落盘的 grid，缺失（老 JSON/首次处理）就现场解析 content。
/// 解析失败 → None（该块不送翻、不覆盖）
fn grid_for(block: &crate::Block) -> Option<crate::table::TableGrid> {
    if block.kind != crate::table::TABLE {
        return None;
    }
    block.grid.clone().or_else(|| crate::table::parse_markup(&block.content))
}

/// 该页是否有"可解析且尚未翻译"的表格（table 在送翻类型里才算）——已翻页的补翻依据
fn has_pending_tables(cfg: &LlmConfig, page: &PageInfo) -> bool {
    page.blocks.iter().any(|b| {
        b.kind == crate::table::TABLE
            && b.translation.is_none()
            && is_translatable(cfg, &b.kind, &b.content)
            && grid_for(b).is_some()
    })
}

/// 收集该页送翻块：送翻类型、content 非空、translation == null
/// （已由邻页 external 绑定的块自动跳过——用户拍板的"不再翻译该文本框"）
fn collect_requests(cfg: &LlmConfig, page: &PageInfo, only_tables: bool) -> Vec<RequestItem> {
    let mut items: Vec<RequestItem> = Vec::new();
    for (pos, b) in page.blocks.iter().enumerate() {
        if !is_translatable(cfg, &b.kind, &b.content) || b.translation.is_some() {
            continue;
        }
        // 已翻页的补翻批次只处理表格；其余类型即使 translation 为空也绝不重发
        // （同语种块被判 null 的就在这里，重发会变成无限回翻）
        if only_tables && b.kind != crate::table::TABLE {
            continue;
        }
        // 表块：标记解析不出网格就整块跳过（不送 token、不覆盖，页面照常标 translated）
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
        "after" => page_index.checked_add(1), // 畸形 JSON 的 u32::MAX 不该 panic
        _ => None,
    }
}

/// 邻页边界候选：before 取末尾 ≤N、after 取开头 ≤N 个送翻块（含已翻译块：
/// 只提供原文参考，外部绑定命中时可覆盖旧值）
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
            // 表块给模型看网格 JSON（送翻负载同形），而不是它读不懂的标记流
            c: match grid_for(b) {
                Some(grid) if b.kind == crate::table::TABLE => crate::table::payload(&grid).to_string(),
                _ => b.content.clone(),
            },
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

/// result 与 requests 严格一一对应（长度 + 每个 index）；A=null 合法（同语种）。
/// 表块额外做形状校验（行数与每行单元格数必须与请求一致，见 table.rs）；
/// 任一表块不合格 → 整页失败（用户 2026-09-16 拍板：整页重来，失败由连败预算兜底回退）
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
            // 表块：A=null 表示整表与目标语言相同 → 保持 null（渲染走原文矩阵）
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

/// 译文写回（A=None 不改：保持 null → 渲染原文 content）。
/// 表块顺手把网格落盘（老 JSON 首次处理后 self-describing，前端只渲染不再解析标记）。
/// 表块的 A=None 例外：整表与目标语言相同 → 落**原文矩阵**当译文（终态）。
/// 留 null 的话，已翻页会被反复选进补翻批次（同语种块无法区分"没翻过"和"不必翻"）。
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
        .map_err(|e| format!("failed to create LLM HTTP client: {e}"))
}

/// opencode zen 端点要求会话路由头（任意非空值即可）；其他 OpenAI 兼容端点
/// 忽略该头。仅在 base_url 命中 opencode.ai 时发送（见 PLAN-LLM.md §5）。
const OPENCODE_SESSION: &str = "ezpdf";

/// 关思考参数（OpenAI Chat Completions）：显式策略 > 实测端点规则 > pi-ai 预设形态（用户 2026-09-15）。
/// - 显式策略来自设置页「验证」探测（用户 2026-09-14），仍然最优先；
/// - opencode zen / SiliconFlow 是实测过的硬规则（见 PLAN-LLM.md §5），不交给预设覆盖；
/// - 预设形态对应 pi-ai openai-completions provider 的 buildParams：deepseek 用
///   thinking.type、zai/qwen 用 enable_thinking、openrouter 用 reasoning.effort、
///   openai 仅在目录给了 off 值时才写 reasoning_effort；"none" = 不加参数。
/// 返回是否真的写入了关思考参数（新协议的 temperature 兼容性判定要用）。
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
    // 预设标记为非思考模型：pi-ai 同样不写任何思考参数
    if cfg.model_reasoning == Some(false) {
        return false;
    }
    // auto：先走实测端点规则
    if is_opencode(&url) {
        // opencode zen（OpenRouter 系）
        body["reasoning"] = json!({"enabled": false});
        body["reasoning_effort"] = json!("none");
        return true;
    }
    if url.contains("siliconflow") {
        // SiliconFlow（Qwen3.5 默认开思考）
        body["enable_thinking"] = json!(false);
        body["thinking"] = json!({"type": "disabled"});
        return true;
    }
    // auto：pi-ai 预设形态
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

/// 关思考（Anthropic Messages）：关掉思考只有一种写法 `thinking.type=disabled`
/// （pi-ai 的 buildParams 在 thinkingEnabled=false 时同样写这个）。思考是 opt-in，
/// 所以非思考模型不必写、显式 "none" 也不写（有些 Anthropic 兼容端点不认这个字段，
/// 验证按钮会把策略落到 "none" 上）。
fn apply_thinking_off_messages(body: &mut Value, cfg: &LlmConfig) -> bool {
    if cfg.thinking_off.trim() == "none" || cfg.model_reasoning == Some(false) {
        return false;
    }
    body["thinking"] = json!({"type": "disabled"});
    true
}

/// 关思考（OpenAI Responses）：`reasoning.effort`（字段位置与 Chat 的顶层
/// reasoning_effort 不同）。目录给了 off 值就用它，否则交给显式策略写 "none"
/// （GPT-5.1+ 支持 effort=none；更早的推理模型目录标 `off: null` = 关不掉，
/// 此时不写任何参数，验证按钮会报告"关不掉"）。
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

/// 模型是否可能仍在思考（预设说会思考、我们也没写成关思考参数）——
/// 两种新协议下 temperature 与思考模式互斥（Anthropic 直接 400，OpenAI 推理模型拒绝），
/// 这种时候就不写 temperature 了；未知模型（自定义端点）不预判，照常写。
fn may_still_think(cfg: &LlmConfig, off_applied: bool) -> bool {
    cfg.model_reasoning == Some(true) && !off_applied
}

/// 拆出系统提示词（Anthropic 是顶层 system 字段、Responses 是顶层 instructions，
/// 都不在 messages 里）；其余消息归一化成 {role, content: 文本}——两家都接受纯字符串
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

/// Responses 的 input 项：user 用 input_text、assistant 用 output_text（pi-ai 同款）。
/// 系统提示词已经进了 instructions，这里跳过。
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

/// 请求体（按协议构造）+ 是否写入了关思考参数。
/// max tokens：Chat 用预设判定的字段名、Messages 是必填的 max_tokens、
/// Responses 是 max_output_tokens。
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
                // 翻译内容不留在服务端（OpenAI 官方字段；pi-ai 也这么发）
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

/// 一次对话请求（协议校验 + 请求体 + 认证头 + zen 路由头 + 预设模型特有头），
/// 供对话与验证共用
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
        // Anthropic 用 x-api-key（不是 bearer）+ 必填的版本头
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

/// 一次非流式回复的协议无关视图（取文本 + 判断是否仍在思考，验证按钮与翻译链共用）
#[derive(Debug)]
struct Completion {
    /// 回复文本（各协议取法不同；None = 响应里没有文本字段）
    text: Option<String>,
    /// 仍在思考：有思考块/思考项，或文本为空（思考吃满了 max_tokens）
    thinks: bool,
}

/// 按协议从响应体取文本与思考标记（非流式）
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
            // thinking / redacted_thinking 块 = 这次仍在思考
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
            // reasoning 项 = 这次仍在思考
            let thinking = output.iter().any(|item| item["type"].as_str() == Some("reasoning"));
            Ok(Completion {
                text: Some(text.clone()),
                thinks: thinking || text.trim().is_empty(),
            })
        }
    }
}

/// 发一次对话请求，返回协议无关的回复视图（取文本与思考检测共用）
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

/// 非流式对话；返回回复文本（协议无关——思考检测等线协议细节都在 chat_raw 里消化）
async fn chat(client: &reqwest::Client, cfg: &LlmConfig, messages: &[Value]) -> Result<String, String> {
    let reply = chat_raw(client, cfg, messages, MAX_TOKENS).await?;
    reply
        .text
        .ok_or_else(|| "LLM response has no text content".to_string())
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
    /// 生效策略："none" = 不写任何关思考参数（要么探测发现不需要、要么都关不掉，
    /// 靠 [`Self::thinking_on`] 区分）
    pub strategy: String,
    /// 预设标记为非思考模型（此时 thinking_on 恒为 false）
    pub preset_no_thinking: bool,
    /// 探测请求里仍有思考内容：关不掉（前端据此 toast 警告）
    pub thinking_on: bool,
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

/// 验证 LLM：连通性 + 关思考策略。
/// 候选策略按协议给（见下），任意一个既连通又不再思考即成功，按候选顺序取第一个；
/// 都发不出去才算连通性失败，都通但都还在思考 → strategy="none"（前端 toast 警告）。
/// 预设模型若标记为非思考（pi-ai 目录 reasoning=false）则只探一次连通性，不做策略搜索。
pub async fn verify_llm(cfg: &LlmConfig) -> Result<LlmVerifyReport, String> {
    if !cfg.usable() {
        return Err("please fill in Base URL / API Key / model first".into());
    }
    let protocol = Protocol::from_api(&cfg.api, &cfg.model)?;
    let client = llm_client()?;
    let messages = vec![json!({"role": "user", "content": PROBE_PROMPT})];
    let started = std::time::Instant::now();
    let preset_no_thinking = cfg.model_reasoning == Some(false);

    // 引用转成 Copy 的绑定，async move 块才能各自捕获（每个候选请求自己持有一份 cfg）
    let ref_client = &client;
    let ref_messages = &messages;
    let probe = |strategy: &str| {
        let mut p = cfg.clone();
        p.thinking_off = strategy.to_string();
        async move { chat_raw(ref_client, &p, ref_messages, PROBE_MAX_TOKENS).await }
    };

    if preset_no_thinking {
        probe("auto").await?; // 只查连通性：非思考模型无需挑策略
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

    // 候选策略：Chat 的四形态是历史（用户 2026-09-15 实测过的几个字段名）；
    // Messages 只有"写 thinking.type=disabled"或"什么都不写"两种；Responses 除了目录给
    // 的 off 值，还留一个显式的 effort=none（GPT-5.1+ 支持，而目录里标的是 off: null）。
    let candidates: &[&str] = match protocol {
        Protocol::Chat => &["auto", "reasoning", "enable_thinking", "thinking_type"],
        Protocol::Messages => &["auto", "none"],
        Protocol::Responses => &["auto", "reasoning", "none"],
    };

    // Chat 的四个候选一次性并发（用户 2026-09-15：串行探测太慢）；其余协议的候选少
    // （最多 3 个）且第一个通常就成功，串行短路省请求
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
                    break; // 已经关掉思考：后面的候选不必再发
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
                // 取候选顺序里第一个"关掉思考"的成功响应（端点拒绝某参数会直接 4xx）
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

/// 拉取模型列表（chat/responses 用 `/models`，Anthropic 用 `/v1/models` + x-api-key）
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
pub fn build_task(cfg: &LlmConfig, doc: &BindDoc, page_index: u32) -> Option<PageTask> {
    let page = doc.pages.iter().find(|p| p.index == page_index)?;
    if !page.finished {
        return None;
    }
    // 已翻页（用户 2026-09-16）：表格支持之前的页面里，表块从未被送翻过（translation=null）。
    // 这类页面允许**只补翻表格**——其余类型一律不动，"已翻页不回翻"依旧成立。
    // 只挑可解析出网格的表格，解析不了的留着不动（前端也不会选它）。
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

/// 翻译禁用路径的落盘（锁内调用，用户 2026-09-15）：把送翻块的 content 直接写成
/// translation 并标记页面 translated，返回是否有变更。
///
/// 语义要点：**不是**留 null。null 表示"待翻译"，重新打开翻译后会被回翻；这里要的是
/// "这批 OCR 已处理完、内容即原文"的终态，所以译文列必须落值。
/// 送翻块集合与联网路径完全一致（is_translatable + translation 为空），非送翻类型
/// （image 等）保持 null——它们本来就没有覆盖框。
/// 表块（用户 2026-09-16）：落**原文矩阵 JSON**（不是标记流）并顺手写网格，这样关着翻译
/// 也能在译文栏看到表格；标记解析失败的表格照旧不翻不覆盖。
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
    page.translated = true; // 页级标记也算进展：整页无送翻块时不该被反复重试
    true
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
                    "p{page_index} round {} {ms}ms output unparsable → correction retry",
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

        // 最终输出：校验 → 返回结果
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
    fn prompt_dir_priority_env_resource_dev() {
        let dev = std::path::Path::new("/dev-repo/system_prompt");
        let res = std::path::Path::new("/usr/lib/ezpdf/system_prompt");
        // env 覆盖最高（含去空白）
        assert_eq!(pick_prompt_dir(Some("  /custom  "), Some(res), dev), PathBuf::from("/custom"));
        // 其次安装包资源目录（生产路径）
        assert_eq!(pick_prompt_dir(None, Some(res), dev), res);
        // 空白 env 视为未设置
        assert_eq!(pick_prompt_dir(Some("   "), Some(res), dev), res);
        // 都没有 → dev 仓库根（单测/源码态）
        assert_eq!(pick_prompt_dir(None, None, dev), dev);
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
        apply_thinking_off_chat(&mut body, "https://x/v1", &explicit("reasoning"));
        assert_eq!(body["reasoning"]["enabled"], json!(false));
        assert_eq!(body["reasoning_effort"], json!("none"));

        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://x/v1", &explicit("enable_thinking"));
        assert_eq!(body["enable_thinking"], json!(false));

        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://x/v1", &explicit("thinking_type"));
        assert_eq!(body["thinking"]["type"], json!("disabled"));

        // auto 矩阵（实测过的端点规则）：zen → reasoning；siliconflow → enable_thinking+thinking
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

        // 未知端点不加参数；显式 none 同样不加
        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://api.openai.com/v1", &auto("", None, None));
        assert!(body.as_object().unwrap().is_empty());
        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://api.openai.com/v1", &explicit("none"));
        assert!(body.as_object().unwrap().is_empty());

        // pi-ai 预设形态（用户 2026-09-15）：deepseek / zai / qwen / openrouter / openai
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
        assert_eq!(body["reasoning_effort"], json!("none")); // openrouter 缺省 off = "none"
        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://x/v1", &auto("reasoning_effort", Some("low"), Some(true)));
        assert_eq!(body["reasoning_effort"], json!("low"));
        // 预设 openai 形态且目录没给 off 值 → 不加参数（与 pi-ai 一致）
        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://x/v1", &auto("none", None, Some(true)));
        assert!(body.as_object().unwrap().is_empty());
        // 预设标记为非思考模型 → 不加参数（即使形态表有值）
        let mut body = json!({});
        apply_thinking_off_chat(&mut body, "https://x/v1", &auto("enable_thinking", None, Some(false)));
        assert!(body.as_object().unwrap().is_empty());

        // 协议暂不支持：请求直接失败（不进 body 构造）
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

    /// Messages / Responses 的关思考：前者固定写 thinking.type，后者写 reasoning.effort
    /// （目录给了 off 值就用它，否则只在显式策略下写 "none"），非思考模型一律不写
    #[test]
    fn thinking_off_shapes_follow_protocol() {
        let messages = |strategy: &str, reasoning: Option<bool>| think_cfg(strategy, "", None, reasoning);
        // Messages：auto / 显式字段名都落到同一个写法
        for strategy in ["auto", "thinking_type", "reasoning"] {
            let mut body = json!({});
            assert!(apply_thinking_off_messages(&mut body, &messages(strategy, Some(true))));
            assert_eq!(body["thinking"]["type"], json!("disabled"));
        }
        // Messages：显式 none / 非思考模型都不写
        let mut body = json!({});
        assert!(!apply_thinking_off_messages(&mut body, &messages("none", Some(true))));
        let mut body = json!({});
        assert!(!apply_thinking_off_messages(&mut body, &messages("auto", Some(false))));
        assert!(body.as_object().unwrap().is_empty());

        // Responses：目录给 off 值 → 用目录值；显式策略且无目录值 → "none"
        let mut body = json!({});
        assert!(apply_thinking_off_responses(&mut body, &think_cfg("auto", "reasoning_effort", Some("minimal"), Some(true))));
        assert_eq!(body["reasoning"]["effort"], json!("minimal"));
        let mut body = json!({});
        assert!(apply_thinking_off_responses(&mut body, &think_cfg("reasoning", "none", None, Some(true))));
        assert_eq!(body["reasoning"]["effort"], json!("none"));
        // Responses：auto 且目录没给 off 值（GPT-5 目录标 off: null）→ 不写
        let mut body = json!({});
        assert!(!apply_thinking_off_responses(&mut body, &think_cfg("auto", "none", None, Some(true))));
        assert!(body.as_object().unwrap().is_empty());
        let mut body = json!({});
        assert!(!apply_thinking_off_responses(&mut body, &think_cfg("none", "reasoning_effort", Some("none"), Some(true))));
        assert!(body.as_object().unwrap().is_empty());
    }

    /// 三个协议的请求体形状：路径 / 系统提示词位置 / max tokens 字段 / 认证头 /
    /// temperature 与思考模式的互斥
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

        // Chat：system 留在 messages 里、max tokens 字段名跟预设、带 bearer
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

        // Messages：system 提到顶层、非 system 消息保留顺序、max_tokens 必填、
        // x-api-key + anthropic-version、url 补 /v1/messages
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
        // 关思考写 thinking.type，于是 temperature 可以照写
        assert_eq!(body["thinking"]["type"], json!("disabled"));
        assert_eq!(body["temperature"], json!(0));
        // 同样的模型+策略 "none"：thinking 关不掉 → 不写 temperature（Anthropic 会 400）
        let no_off = LlmConfig { thinking_off: "none".into(), ..msg_cfg.clone() };
        assert!(body_of(&no_off).get("temperature").is_none());
        assert_eq!(body_of(&no_off)["thinking"], Value::Null);

        // Responses：instructions + input 项（user=input_text / assistant=output_text）、
        // max_output_tokens、bearer、url 补 /responses
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
        assert_eq!(body["input"].as_array().unwrap().len(), 3); // system 不在 input 里
        // 目录没给 off 值 → thinking 关不掉 → 不写 temperature（推理模型会拒绝）
        assert!(body.get("temperature").is_none());
        assert_eq!(body_of(&LlmConfig { thinking_off: "reasoning".into(), ..resp_cfg.clone() })["reasoning"]["effort"], json!("none"));
    }

    /// 协议映射与 URL 归一化（自定义端点最容易把 /v1 也填进来）
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

    /// 响应取字段：Chat=choices[0].message、Messages=content[] 文本块、
    /// Responses=output[] 的 message/output_text；思考标记各协议各认各的
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

        // Messages：文本块拼接，thinking 块 = 仍在思考
        let msgs = json!({"content": [{"type": "thinking", "thinking": "想…"}, {"type": "text", "text": "4"}]});
        let c = completion_of(Protocol::Messages, &msgs, "{}").unwrap();
        assert_eq!(c.text.as_deref(), Some("4"));
        assert!(c.thinks);
        let msgs_off = json!({"content": [{"type": "text", "text": "4"}]});
        assert!(!completion_of(Protocol::Messages, &msgs_off, "{}").unwrap().thinks);
        // 只有思考块（文本为空）也算还在思考
        let only_think = json!({"content": [{"type": "thinking", "thinking": "想"}]});
        assert!(completion_of(Protocol::Messages, &only_think, "{}").unwrap().thinks);
        assert!(completion_of(Protocol::Messages, &json!({}), "{}").unwrap_err().contains("content[]"));

        // Responses：reasoning 项 = 仍在思考
        let resp = json!({"output": [
            {"type": "reasoning", "summary": [{"type": "summary_text", "text": "想"}]},
            {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "4"}]},
        ]});
        let c = completion_of(Protocol::Responses, &resp, "{}").unwrap();
        assert_eq!(c.text.as_deref(), Some("4"));
        assert!(c.thinks);
        let resp_off = json!({"output": [{"type": "message", "content": [{"type": "output_text", "text": "4"}]}]});
        assert!(!completion_of(Protocol::Responses, &resp_off, "{}").unwrap().thinks);
        // refusal 部分不是文本、空文本也算还在思考
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

    /// 表块送翻（用户 2026-09-16）：Q 是 {"table": [[...]]} 网格负载；标记解析不出形状的
    /// 表格整块跳过（不送 token、不覆盖，页面照常被标记完成）
    #[test]
    fn collect_requests_packs_tables_and_skips_unparsable() {
        // 类型白名单：table + text（未列入的类型即使 content 非空也不送翻）
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

    /// 表块结果形状校验（用户 2026-09-16）：形状不符 → 整页失败（连败预算兜底回退）；
    /// A=null → 整表与目标语言相同，保持 null
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

    /// 表块落盘：译文写入二维矩阵 JSON，同时把网格写进绑定 JSON（前端只渲染不再解析标记）
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

    /// 翻译禁用路径的表块：落原文矩阵 + 网格（不是标记流），解析失败的表格保持 null
    /// 已翻页的表格补翻（用户 2026-09-16）：只送未翻且有网格的表格，
    /// 同语种的空译文块绝不重发（否则无限回翻）
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
        // 表格标记解析不出网格 → 没有待补翻目标 → 整页不做
        let unparsable = page(1, vec![blk("table", "没有表格标记", None)], true);
        assert!(build_task(&table_cfg, &doc(vec![unparsable]), 1).is_none());
        // 表格已翻 → 不做
        let done = page(1, vec![blk("table", "<fcel>a<nl>", Some("[[\"A\"]]"))], true);
        assert!(build_task(&table_cfg, &doc(vec![done]), 1).is_none());
        // table 不在送翻类型里 → 不做
        let no_table = LlmConfig { translate_types: vec!["text".into()], ..cfg(true) };
        let pending = page(1, vec![blk("table", "<fcel>a<nl>", None)], true);
        assert!(build_task(&no_table, &doc(vec![pending]), 1).is_none());
    }

    /// 表块 A=null（整表与目标语言相同）→ 落原文矩阵当译文（终态，不会再被选进补翻批次）
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

    /// 翻译禁用路径（用户 2026-09-15）：原文当译文落盘、页标记完成；
    /// 非送翻类型保持 null（它们本来就没有覆盖框），已翻页不动
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
        // 幂等：已翻页再跑不产生变更
        assert!(!apply_bypass(&mut d, 1, &cfg(true)));
        // 未 OCR 完成的页不处理
        let mut pending = doc(vec![PageInfo { finished: false, ..page(1, vec![blk("text", "x", None)], false) }]);
        assert!(!apply_bypass(&mut pending, 1, &cfg(true)));
    }

    /// 送翻类型可配（用户 2026-09-15）：非空集合覆盖内置默认；空集合回落默认
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
        // before（邻页 2）：取末尾 ≤3 个送翻块
        let b = context_candidates(&cfg(true), &d, 2, "before");
        assert_eq!(b.len(), 3);
        assert_eq!(b[0].c, "t3");
        assert_eq!(b[2].c, "t5");
        // after（邻页 2）：取开头 ≤3 个送翻块
        let a = context_candidates(&cfg(true), &d, 2, "after");
        assert_eq!(a.len(), 3);
        assert_eq!(a[0].c, "t1");
        assert_eq!(a[2].c, "t3");
        // 越界页
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
        assert!(build_task(&cfg(true), &d, 2).is_none());
        assert!(build_task(&cfg(true), &d, 1).is_none());
        // 正常页：before 取 p2（虽有块但未送翻候选？p2 有 text 块 → 候选 1），after 取 p4（无页 → 空候选）
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
