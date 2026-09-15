//! pyserver 生命周期状态机（唯一持有者）：
//! unknown → starting → connected / failed(冷启超时 60s、连崩 3 次终态)；
//! 断线自动重启（指数退避）；停止与应用退出经关 stdin 触发 Python 自灭（stdin-EOF 防孤儿）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::watch;
use ts_rs::TS;

use crate::pyenv;
use crate::pyenv::PyPaths;

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 就绪行前缀：`EZPDF_READY {"port":...,"pid":...}`（stdout 单行 JSON）
const READY_PREFIX: &str = "EZPDF_READY ";
/// 冷启上限（READY 行等待）
const START_TIMEOUT: Duration = Duration::from_secs(60);
/// 连续崩溃终态阈值
const MAX_CRASHES: u32 = 3;
/// 主动停止后等待 Python 自灭的上限，超时强杀
const STOP_GRACE: Duration = Duration::from_secs(8);
/// 自动重启退避上限
const BACKOFF_CAP: Duration = Duration::from_secs(15);

/// 服务生命周期状态（前端「服务」灯数据源）；serde 序列化为小写
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum ServiceStatus {
    Unknown,
    Starting,
    Connected,
    Disconnected,
    Failed,
}

struct PyServiceInner {
    status: Mutex<ServiceStatus>,
    /// 主动停止/应用退出标记；重启循环见到它即退出
    stopping: AtomicBool,
    /// 持有子进程 stdin 不关是防孤儿的前提；stop() 主动 drop 触发 Python EOF 自灭
    stdin: Mutex<Option<tokio::process::ChildStdin>>,
    /// 最近一次成功握手的 base URL 与 token（阶段4 OCR 调用取用）
    endpoint: Mutex<Option<String>>,
    token: Mutex<String>,
    stop_tx: watch::Sender<bool>,
    stop_rx: watch::Receiver<bool>,
}

/// 状态机共享柄（Clone = 内部 Arc 引用复制，代价可忽略）
#[derive(Clone)]
pub struct PyService(Arc<PyServiceInner>);

impl PyService {
    pub fn new() -> Self {
        let (stop_tx, stop_rx) = watch::channel(false);
        Self(Arc::new(PyServiceInner {
            status: Mutex::new(ServiceStatus::Unknown),
            stopping: AtomicBool::new(false),
            stdin: Mutex::new(None),
            endpoint: Mutex::new(None),
            token: Mutex::new(String::new()),
            stop_tx,
            stop_rx,
        }))
    }

    pub fn status(&self) -> ServiceStatus {
        self.0.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn set_status(&self, app: &AppHandle, status: ServiceStatus) {
        *self.0.status.lock().unwrap_or_else(|e| e.into_inner()) = status.clone();
        let _ = app.emit("ocr://status", status);
    }

    fn set_endpoint(&self, base: String, token: String) {
        *self.0.endpoint.lock().unwrap_or_else(|e| e.into_inner()) = Some(base);
        *self.0.token.lock().unwrap_or_else(|e| e.into_inner()) = token;
    }

    /// OCR 调用取用点（阶段4）：握手成功后的 (base, token)；未连接 → None
    pub fn ocr_target(&self) -> Option<(String, String)> {
        let base = self.0.endpoint.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
        let token = self.0.token.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Some((base, token))
    }

    /// 在线模式（用户 2026-09-15）：把远端解析服务登记为 OCR 目标（无会话 token）。
    /// 探测通过由调用方负责；这里只落端点 + 置 Connected（前端「服务」灯靠它）
    pub fn set_remote(&self, app: &AppHandle, base: String) {
        self.set_endpoint(base, String::new());
        self.set_status(app, ServiceStatus::Connected);
    }

    /// 在线模式断开：清端点 + 复位状态（无子进程，不碰 stdin / supervisor）
    pub fn disconnect(&self, app: &AppHandle) {
        *self.0.endpoint.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.0.token.lock().unwrap_or_else(|e| e.into_inner()) = String::new();
        self.set_status(app, ServiceStatus::Unknown);
    }

    /// 是否托管着本地子进程（在线模式没有）；ocr_stop 据此选择断开方式
    pub fn has_child(&self) -> bool {
        self.0.stdin.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    /// 停止（同步）：置标记 + 通知 supervisor + 关 stdin（Python 优雅退出）。
    /// 供 RunEvent::Exit（同步上下文）与 ocr_stop 命令共用。
    pub fn stop(&self) {
        self.0.stopping.store(true, Ordering::SeqCst);
        let _ = self.0.stop_tx.send(true);
        drop(self.0.stdin.lock().unwrap_or_else(|e| e.into_inner()).take()); // EOF → python 自灭
    }

    /// 门控：Starting/Connected 期间忽略重复 ocr_start；Unknown/Disconnected/Failed 可拉起
    pub fn startable(&self) -> bool {
        !matches!(self.status(), ServiceStatus::Starting | ServiceStatus::Connected)
    }

    pub fn reset_for_start(&self) {
        self.0.stopping.store(false, Ordering::SeqCst);
        let _ = self.0.stop_tx.send(false);
    }
}

/// 随机会话 token（防本地误连即可，非密码学用途）。/// RandomState 的密钥取自 OS 熵（sys::hashmap_random_keys），比"时间+pid"可预测种子强。
fn fresh_token() -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hash, Hasher};
    let mut seed = RandomState::new().build_hasher();
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        .hash(&mut seed);
    std::process::id().hash(&mut seed);
    let extra = RandomState::new().build_hasher().finish();
    format!("{:016x}{:016x}", seed.finish(), extra)
}

/// supervisor 主循环：spawn → READY → /health → connected → 等退出；
/// 退出且非主动停止 → 崩溃计数 → 退避重启；连崩 3 次 → failed 终态。
pub async fn supervise(app: AppHandle, paths: PyPaths, svc: PyService) {
    let mut backoff = Duration::from_secs(1);
    let mut crashes: u32 = 0;
    loop {
        if svc.0.stopping.load(Ordering::SeqCst) {
            svc.set_status(&app, ServiceStatus::Unknown);
            break; // 停止请求（可能发生在退避等待期间）：复位为未启动再收尾
        }
        svc.set_status(&app, ServiceStatus::Starting);
        match start_once(&app, &paths, &svc).await {
            Ok(mut child) => {
                svc.set_status(&app, ServiceStatus::Connected);
                backoff = Duration::from_secs(1); // 成功过一次即重置退避
                let mut stop_rx = svc.0.stop_rx.clone();
                stop_rx.borrow_and_update(); // 标记当前值已读，changed() 只等下一次
                let _exit = tokio::select! {
                    st = child.wait() => st,
                    _ = stop_rx.changed() => {
                        // stdin 已由 stop() 关闭：等 Python 优雅退出，超时强杀兜底
                        match tokio::time::timeout(STOP_GRACE, child.wait()).await {
                            Ok(st) => st,
                            Err(_) => {
                                let _ = child.start_kill();
                                child.wait().await
                            }
                        }
                    }
                };
                if svc.0.stopping.load(Ordering::SeqCst) {
                    svc.set_status(&app, ServiceStatus::Unknown);
                    break; // 主动停止 / 应用退出：状态复位为未启动，允许再次拉起
                }
                if handle_failure(&app, &svc, &mut crashes, &mut backoff, None).await {
                    break;
                }
            }
            Err(err) => {
                if handle_failure(&app, &svc, &mut crashes, &mut backoff, Some(&err)).await {
                    break;
                }
            }
        }
    }
}

/// 一次启动失败/异常退出后的统一善后：崩溃计数 → 终态或退避等待。
/// 返回 true = 已达终态（调用方 break）。
async fn handle_failure(
    app: &AppHandle,
    svc: &PyService,
    crashes: &mut u32,
    backoff: &mut Duration,
    err: Option<&str>,
) -> bool {
    *crashes += 1;
    if *crashes >= MAX_CRASHES {
        svc.set_status(app, ServiceStatus::Failed);
        if let Some(e) = err {
            let _ = app.emit("ocr://log", format!("[ezpdf] service start failed: {e}"));
        }
        return true;
    }
    svc.set_status(app, ServiceStatus::Disconnected);
    tokio::time::sleep(*backoff).await;
    *backoff = (*backoff * 2).min(BACKOFF_CAP);
    false
}

/// 单次启动：spawn → 逐行读 stdout 等 READY（超时 60s）→ /health 确认 → 持 stdin
async fn start_once(app: &AppHandle, paths: &PyPaths, svc: &PyService) -> Result<tokio::process::Child, String> {
    if !paths.python.is_file() {
        return Err("python interpreter not found; run \"Install service\" in Settings".into());
    }
    let token = fresh_token();
    let mut cmd = tokio::process::Command::new(&paths.python);
    cmd.args(["-m", "app.main"])
        .current_dir(&paths.root)
        .env("EZPDF_TOKEN", &token)
        .env("EZPDF_MODELS_DIR", &paths.models)
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let mut child = cmd.spawn().map_err(|e| format!("failed to spawn python: {e}"))?;

    // stdin 存入全局状态：select 主分支持有 child，stop() 拿走 stdin 触发 EOF
    if let Some(stdin) = child.stdin.take() {
        *svc.0.stdin.lock().unwrap_or_else(|e| e.into_inner()) = Some(stdin);
    }

    // stderr → ocr://log（uvicorn / 引擎日志）
    if let Some(mut stderr) = child.stderr.take() {
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            pyenv::forward_lines(&mut stderr, &app2).await;
        });
    }

    // stdout 逐行读：READY 行经 oneshot 回报端口，其余行进日志
    let stdout = child.stdout.take().ok_or("stdout pipe missing")?;
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Option<u16>>();
    let app3 = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut buf = BufReader::new(stdout);
        let mut line = String::new();
        let mut ready_tx = Some(ready_tx);
        loop {
            line.clear();
            match buf.read_line(&mut line).await {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let t = line.trim_end();
                    if let Some(rest) = t.strip_prefix(READY_PREFIX) {
                        let port = serde_json::from_str::<serde_json::Value>(rest)
                            .ok()
                            .and_then(|v| v["port"].as_u64())
                            .map(|p| p as u16);
                        if let Some(tx) = ready_tx.take() {
                            let _ = tx.send(port);
                        }
                        break;
                    } else if !t.is_empty() {
                        let _ = app3.emit("ocr://log", t.to_string());
                    }
                }
            }
        }
        if let Some(tx) = ready_tx.take() {
            let _ = tx.send(None); // EOF 未见到 READY
        }
    });

    let port = match tokio::time::timeout(START_TIMEOUT, ready_rx).await {
        Ok(Ok(Some(port))) => port,
        Ok(Ok(None)) => {
            let _ = child.start_kill();
            return Err("process exited before READY".into());
        }
        Ok(Err(_)) => {
            let _ = child.start_kill();
            return Err("failed to read stdout".into());
        }
        Err(_) => {
            let _ = child.start_kill();
            return Err("startup timeout (no READY line within 60s)".into());
        }
    };

    // /health 轻量复核（READY 已意味着服务在跑）
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    for _ in 0..5 {
        if let Ok(resp) = client
            .get(format!("{base}/health"))
            .header("x-ezpdf-token", &token)
            .send()
            .await
        {
            if resp.status().is_success() {
                svc.set_endpoint(base, token);
                return Ok(child);
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let _ = child.start_kill();
    Err("health check failed".into())
}

// ---- 在线模式（用户 2026-09-15）：远端解析服务探活/登记 ----
//
// 与本地托管服务的区别：没有子进程可管、没有会话 token（远端由部署方决定要不要鉴权，
// 文档见 pyserver/PROTOCOL.md）；连接期只有一次 /health 探测。开发联调用
// `pyserver/server_test.py`（默认 127.0.0.1:9055）起一个本地解析服务当"远端"。

/// 探活超时：/health 是纯内存响应，超过这个时间说明地址/网络不对
const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);

/// 规范化用户输入的地址：去空白/尾斜杠，缺协议按 http 补（本地开发最常见）。
/// 空串报错（避免把空地址当成"连上了"）。
pub fn normalize_base(url: &str) -> Result<String, String> {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("parse service URL is empty".into());
    }
    let with_scheme = if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    // 只接受 http(s)，避免把 file:// 之类当服务地址
    if !(with_scheme.starts_with("http://") || with_scheme.starts_with("https://")) {
        return Err(format!("unsupported parse service URL: {with_scheme}"));
    }
    Ok(with_scheme)
}

/// 回环地址：系统代理不该插手本机服务（与 Rust 侧本地托管的直连语义一致）
fn is_loopback(base: &str) -> bool {
    let authority = base.split("://").nth(1).unwrap_or(base);
    let authority = authority.split(['/', '?']).next().unwrap_or("");
    let hostport = authority.rsplit('@').next().unwrap_or(authority);
    let host = match hostport.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""), // IPv6 字面量 [::1]:9055
        None => hostport.split(':').next().unwrap_or(""),
    };
    matches!(host, "localhost" | "127.0.0.1" | "0.0.0.0" | "::1")
}

/// 在线解析服务的握手结果（在线模式「测试」按钮 + 每次 OCR 请求前的批大小协商）。
/// `max_batch_pages` 由服务端公布（PROTOCOL.md §4），已夹到 1..=32
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ParseServiceHealth {
    pub status: String,
    /// u32 而非 u64：ts-rs 会把 u64 映射成 bigint，前端只想拿它显示
    pub pid: Option<u32>,
    /// 服务端允许的单批最大页数
    pub max_batch_pages: u32,
    /// 本次探活耗时（ms）
    pub elapsed_ms: u32,
}

/// 服务端没公布/公布得不可信时的批大小：与客户端本地托管的批大小一致
pub const FALLBACK_BATCH_PAGES: u32 = 4;

/// 解析服务探活：GET {base}/health → (pid, 耗时 ms, 服务端公布的批大小)。失败给出可读
/// 原因（前端「测试」按钮、在线模式连接前确认、每次 OCR 请求前的批大小握手共用）。
pub async fn probe_health(base: &str) -> Result<ParseServiceHealth, String> {
    let base = normalize_base(base)?;
    let url = format!("{base}/health");
    let mut builder = reqwest::Client::builder().timeout(HEALTH_TIMEOUT);
    if is_loopback(&base) {
        builder = builder.no_proxy();
    }
    let client = builder.build().map_err(|e| format!("failed to create HTTP client: {e}"))?;
    let started = std::time::Instant::now();
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("cannot reach {url}: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("{url} answered HTTP {status}"));
    }
    let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
    Ok(ParseServiceHealth {
        status: body["status"].as_str().unwrap_or("ok").to_string(),
        pid: body["pid"].as_u64().map(|p| p.min(u32::MAX as u64) as u32),
        max_batch_pages: advertised_batch_pages(&body),
        elapsed_ms: started.elapsed().as_millis().min(u32::MAX as u128) as u32,
    })
}

/// 服务端公布的批大小 → 客户端可用值：缺失/非法回落 4，越界夹进 1..=32。
/// 夹上界是硬要求——本地 Rust 侧的批次上限就是 32，超了整批会被拒
fn advertised_batch_pages(body: &serde_json::Value) -> u32 {
    match body["max_batch_pages"].as_u64() {
        Some(n) if n >= 1 => (n.min(32)) as u32,
        _ => FALLBACK_BATCH_PAGES,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_base_fills_scheme_and_rejects_junk() {
        assert_eq!(normalize_base(" 127.0.0.1:9055/ ").unwrap(), "http://127.0.0.1:9055");
        assert_eq!(
            normalize_base("https://parse.example.com/api").unwrap(),
            "https://parse.example.com/api"
        );
        assert!(normalize_base("   ").unwrap_err().contains("empty"));
        assert!(normalize_base("file:///etc/passwd").is_err());
    }

    /// 回环判定：决定探活请求是否绕开系统代理
    /// 批大小协商：服务端缺失/非法值回落 4，越界夹进 1..=32（本地 Rust 上限 32）
    #[test]
    fn advertised_batch_pages_is_clamped() {
        assert_eq!(advertised_batch_pages(&serde_json::json!({"max_batch_pages": 6})), 6);
        assert_eq!(advertised_batch_pages(&serde_json::json!({"max_batch_pages": 9999})), 32);
        assert_eq!(advertised_batch_pages(&serde_json::json!({"max_batch_pages": 0})), 4);
        assert_eq!(advertised_batch_pages(&serde_json::json!({})), 4);
        assert_eq!(advertised_batch_pages(&serde_json::json!({"max_batch_pages": "x"})), 4);
    }

    #[test]
    fn loopback_hosts_are_recognized() {
        assert!(is_loopback("http://127.0.0.1:9055"));
        assert!(is_loopback("http://localhost:9055/health"));
        assert!(is_loopback("http://[::1]:9055"));
        assert!(is_loopback("http://user:pw@localhost:9055"));
        assert!(!is_loopback("https://parse.example.com"));
        assert!(!is_loopback("http://10.0.0.7:9055"));
    }
}
