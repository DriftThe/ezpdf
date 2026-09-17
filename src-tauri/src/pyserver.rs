//! pyserver lifecycle state machine (sole owner): unknown → starting → connected/failed (60 s cold
//! start, 3 crashes = terminal, exponential backoff); stop closes stdin so Python's EOF watchdog exits.

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

/// Ready-line protocol: `EZPDF_READY {"port":...,"pid":...}` on stdout.
const READY_PREFIX: &str = "EZPDF_READY ";
const START_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_CRASHES: u32 = 3;
/// Grace period for Python to self-exit after a stop; force-killed on timeout.
const STOP_GRACE: Duration = Duration::from_secs(8);
const BACKOFF_CAP: Duration = Duration::from_secs(15);

/// Service lifecycle status (frontend "service" light source); serde serializes it as camelCase.
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
    /// Stop/exit flag; the restart loop exits on seeing it.
    stopping: AtomicBool,
    /// Holding the child's stdin open prevents orphans; stop() drops it to trigger Python's EOF exit.
    stdin: Mutex<Option<tokio::process::ChildStdin>>,
    endpoint: Mutex<Option<String>>,
    token: Mutex<String>,
    stop_tx: watch::Sender<bool>,
    stop_rx: watch::Receiver<bool>,
}

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

    fn status(&self) -> ServiceStatus {
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

    /// OCR call target: (base, token) after a successful handshake; None when disconnected.
    pub fn ocr_target(&self) -> Option<(String, String)> {
        let base = self.0.endpoint.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
        let token = self.0.token.lock().unwrap_or_else(|e| e.into_inner()).clone();
        Some((base, token))
    }

    /// Register a remote service as the OCR target (caller must have probed); this only stores endpoint+token and sets Connected.
    pub fn set_remote(&self, app: &AppHandle, base: String, token: String) {
        self.set_endpoint(base, token);
        self.set_status(app, ServiceStatus::Connected);
    }

    /// Remote disconnect: clear the endpoint and reset status (no child, no stdin/supervisor).
    pub fn disconnect(&self, app: &AppHandle) {
        *self.0.endpoint.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.0.token.lock().unwrap_or_else(|e| e.into_inner()) = String::new();
        self.set_status(app, ServiceStatus::Unknown);
    }

    /// Whether a local child is managed (remote mode has none); ocr_stop picks its disconnect path from this.
    pub fn has_child(&self) -> bool {
        self.0.stdin.lock().unwrap_or_else(|e| e.into_inner()).is_some()
    }

    /// Stop (sync): set the flag, notify the supervisor, close stdin so Python exits gracefully.
    pub fn stop(&self) {
        self.0.stopping.store(true, Ordering::SeqCst);
        let _ = self.0.stop_tx.send(true);
        drop(self.0.stdin.lock().unwrap_or_else(|e| e.into_inner()).take()); // EOF → python exits on its own
    }

    /// Gate: duplicate ocr_start is ignored while Starting/Connected; Unknown/Disconnected/Failed can start.
    pub fn startable(&self) -> bool {
        !matches!(self.status(), ServiceStatus::Starting | ServiceStatus::Connected)
    }

    pub fn reset_for_start(&self) {
        self.0.stopping.store(false, Ordering::SeqCst);
        let _ = self.0.stop_tx.send(false);
    }
}

/// Random session token (not cryptographic; only guards accidental local connections) seeded from OS entropy.
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

/// Supervisor loop: spawn → READY → /health → connected → wait; an unintentional exit counts a crash and restarts with backoff.
pub async fn supervise(app: AppHandle, paths: PyPaths, svc: PyService) {
    let mut backoff = Duration::from_secs(1);
    let mut crashes: u32 = 0;
    loop {
        if svc.0.stopping.load(Ordering::SeqCst) {
            svc.set_status(&app, ServiceStatus::Unknown);
            break; // stop requested (possibly during backoff): reset to unstarted and finish
        }
        svc.set_status(&app, ServiceStatus::Starting);
        match start_once(&app, &paths, &svc).await {
            Ok(mut child) => {
                svc.set_status(&app, ServiceStatus::Connected);
                backoff = Duration::from_secs(1); // one success resets the backoff
                let mut stop_rx = svc.0.stop_rx.clone();
                stop_rx.borrow_and_update();
                let _exit = tokio::select! {
                    st = child.wait() => st,
                    _ = stop_rx.changed() => {
                        // stdin was closed by stop(): wait for graceful exit, force-kill on timeout
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
                    break; // stop/app exit: reset to unstarted so it can start again
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

/// After a failed start/abnormal exit: bump the crash count, then go terminal or wait out the backoff; true = terminal.
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

/// One start attempt: spawn → read stdout lines for READY (60 s timeout) → /health confirm → hold stdin.
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

    // Store stdin globally so stop() can take it to trigger EOF while the select branch owns the child.
    if let Some(stdin) = child.stdin.take() {
        *svc.0.stdin.lock().unwrap_or_else(|e| e.into_inner()) = Some(stdin);
    }

    if let Some(mut stderr) = child.stderr.take() {
        let app2 = app.clone();
        tauri::async_runtime::spawn(async move {
            pyenv::forward_lines(&mut stderr, &app2, None).await;
        });
    }

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
            let _ = tx.send(None); // EOF before READY
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

    // Light /health recheck (READY already means the service is up).
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

// ---- Remote mode: no child/session token (deployer decides auth, see PROTOCOL.md); connect = one /health probe; server_test.py fakes one on 9055 ----

/// Probe timeout: /health is in-memory, so exceeding this means a wrong address/network.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(5);

/// Normalize a user URL: trim, default a missing scheme to http; empty errors rather than counting as connected.
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
    // Accept only http(s), so file:// etc. can't masquerade as a service URL.
    if !(with_scheme.starts_with("http://") || with_scheme.starts_with("https://")) {
        return Err(format!("unsupported parse service URL: {with_scheme}"));
    }
    Ok(with_scheme)
}

/// Loopback detection: the system proxy shouldn't touch local services (matches local direct connect).
fn is_loopback(base: &str) -> bool {
    let authority = base.split("://").nth(1).unwrap_or(base);
    let authority = authority.split(['/', '?']).next().unwrap_or("");
    let hostport = authority.rsplit('@').next().unwrap_or(authority);
    let host = match hostport.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""), // IPv6 literal [::1]:9055
        None => hostport.split(':').next().unwrap_or(""),
    };
    matches!(host, "localhost" | "127.0.0.1" | "0.0.0.0" | "::1")
}

/// Remote handshake result; `max_batch_pages` comes from the server (PROTOCOL.md §4), clamped to 1..=32.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ParseServiceHealth {
    /// u32 not u64: ts-rs maps u64 to bigint, and the frontend only displays this.
    pub pid: Option<u32>,
    pub max_batch_pages: u32,
    pub elapsed_ms: u32,
}

/// Batch size when the server doesn't advertise a usable value (matches local hosting's size).
const FALLBACK_BATCH_PAGES: u32 = 4;

/// GET {base}/health → (pid, elapsed, batch size); shared by Test, pre-connect confirmation and per-request negotiation.
pub async fn probe_health(base: &str, token: &str) -> Result<ParseServiceHealth, String> {
    let base = normalize_base(base)?;
    let url = format!("{base}/health");
    let mut builder = reqwest::Client::builder().timeout(HEALTH_TIMEOUT);
    if is_loopback(&base) {
        builder = builder.no_proxy();
    }
    let client = builder.build().map_err(|e| format!("failed to create HTTP client: {e}"))?;
    let started = std::time::Instant::now();
    let mut req = client.get(&url);
    if !token.is_empty() {
        req = req.header("x-ezpdf-token", token);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("cannot reach {url}: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        // 403 almost always means a wrong/missing token: give an actionable hint.
        if status.as_u16() == 403 {
            return Err(format!("{url} answered HTTP 403: service token rejected"));
        }
        return Err(format!("{url} answered HTTP {status}"));
    }
    let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
    Ok(ParseServiceHealth {
        pid: body["pid"].as_u64().map(|p| p.min(u32::MAX as u64) as u32),
        max_batch_pages: advertised_batch_pages(&body),
        elapsed_ms: started.elapsed().as_millis().min(u32::MAX as u128) as u32,
    })
}

/// Advertised batch size → usable value: missing/invalid → 4, out-of-range clamps to 1..=32 (Rust rejects over 32).
fn advertised_batch_pages(body: &serde_json::Value) -> u32 {
    match body["max_batch_pages"].as_u64() {
        Some(n) if n >= 1 => (n.min(crate::parse::MAX_BATCH_PAGES as u64)) as u32,
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
