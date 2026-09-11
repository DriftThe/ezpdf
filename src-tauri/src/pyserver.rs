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

/// 随机会话 token（防本地误连即可，非密码学用途）
fn fresh_token() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        .hash(&mut hasher);
    std::process::id().hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// supervisor 主循环：spawn → READY → /health → connected → 等退出；
/// 退出且非主动停止 → 崩溃计数 → 退避重启；连崩 3 次 → failed 终态。
pub async fn supervise(app: AppHandle, paths: PyPaths, svc: PyService) {
    let mut backoff = Duration::from_secs(1);
    let mut crashes: u32 = 0;
    loop {
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
                    break; // 主动停止 / 应用退出：正常收尾
                }
                crashes += 1;
                if crashes >= MAX_CRASHES {
                    svc.set_status(&app, ServiceStatus::Failed);
                    break;
                }
                svc.set_status(&app, ServiceStatus::Disconnected);
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_CAP);
            }
            Err(err) => {
                crashes += 1;
                if crashes >= MAX_CRASHES {
                    svc.set_status(&app, ServiceStatus::Failed);
                    let _ = app.emit("ocr://log", format!("[ezpdf] 服务启动失败: {err}"));
                    break;
                }
                svc.set_status(&app, ServiceStatus::Disconnected);
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(BACKOFF_CAP);
            }
        }
    }
}

/// 单次启动：spawn → 逐行读 stdout 等 READY（超时 60s）→ /health 确认 → 持 stdin
async fn start_once(app: &AppHandle, paths: &PyPaths, svc: &PyService) -> Result<tokio::process::Child, String> {
    if !paths.venv_python.is_file() {
        return Err("venv 解释器不存在，请先在设置页安装环境".into());
    }
    let token = fresh_token();
    let mut cmd = tokio::process::Command::new(&paths.venv_python);
    cmd.args(["-m", "app.main"])
        .current_dir(&paths.root)
        .env("EZPDF_TOKEN", &token)
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let mut child = cmd.spawn().map_err(|e| format!("spawn python 失败: {e}"))?;

    // stdin 存入全局状态：select 主分支持有 child，stop() 拿走 stdin 触发 EOF
    if let Some(stdin) = child.stdin.take() {
        *svc.0.stdin.lock().unwrap() = Some(stdin);
    }

    // stderr → ocr://log（uvicorn / 引擎日志）
    if let Some(stderr) = child.stderr.take() {
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            let mut buf = BufReader::new(stderr);
            let mut line = String::new();
            loop {
                line.clear();
                match buf.read_line(&mut line).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let t = line.trim_end();
                        if !t.is_empty() {
                            let _ = app2.emit("ocr://log", t.to_string());
                        }
                    }
                }
            }
        });
    }

    // stdout 逐行读：READY 行经 oneshot 回报端口，其余行进日志
    let stdout = child.stdout.take().ok_or("stdout 管道缺失")?;
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
            return Err("进程在 READY 前退出".into());
        }
        Ok(Err(_)) => {
            let _ = child.start_kill();
            return Err("stdout 读取失败".into());
        }
        Err(_) => {
            let _ = child.start_kill();
            return Err("启动超时（60s 内未见 READY 行）".into());
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
    Err("健康检查未通过".into())
}
