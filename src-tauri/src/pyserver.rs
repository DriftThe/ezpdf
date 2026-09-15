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
        self.0.status.lock().unwrap().clone()
    }

    fn set_status(&self, app: &AppHandle, status: ServiceStatus) {
        *self.0.status.lock().unwrap() = status.clone();
        let _ = app.emit("ocr://status", status);
    }

    fn set_endpoint(&self, base: String, token: String) {
        *self.0.endpoint.lock().unwrap() = Some(base);
        *self.0.token.lock().unwrap() = token;
    }

    /// OCR 调用取用点（阶段4）：握手成功后的 (base, token)；未连接 → None
    pub fn ocr_target(&self) -> Option<(String, String)> {
        let base = self.0.endpoint.lock().unwrap().clone()?;
        let token = self.0.token.lock().unwrap().clone();
        Some((base, token))
    }

    /// 停止（同步）：置标记 + 通知 supervisor + 关 stdin（Python 优雅退出）。
    /// 供 RunEvent::Exit（同步上下文）与 ocr_stop 命令共用。
    pub fn stop(&self) {
        self.0.stopping.store(true, Ordering::SeqCst);
        let _ = self.0.stop_tx.send(true);
        drop(self.0.stdin.lock().unwrap().take()); // EOF → python 自灭
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

/// 随机会话 token（防本地误连即可，非密码学用途）。
/// RandomState 的密钥取自 OS 熵（sys::hashmap_random_keys），比"时间+pid"可预测种子强。
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
        *svc.0.stdin.lock().unwrap() = Some(stdin);
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
