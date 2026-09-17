use std::collections::BTreeMap;
#[cfg(windows)]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use ts_rs::TS;
#[cfg(not(dev))]
use tauri::Manager; // only the production branch needs resource_dir()/home_dir()

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Install sources for the mirror checkbox: TUNA pypi + SJTU pytorch-wheels/cu132 vs pypi.org / download.pytorch.org.
const PYPI_MIRROR: &str = "https://pypi.tuna.tsinghua.edu.cn/simple";
const PYPI_OFFICIAL: &str = "https://pypi.org/simple";
const TORCH_MIRROR_BASE: &str = "https://mirror.sjtu.edu.cn/pytorch-wheels";
const TORCH_OFFICIAL_BASE: &str = "https://download.pytorch.org/whl";

/// Interpreter executable name (runtime cfg! so both branches compile-check).
fn py_exe_name() -> &'static str {
    if cfg!(windows) {
        "python.exe"
    } else {
        "python3"
    }
}

/// Bundled interpreter under resources/python (Windows python.exe, else bin/python3, python-build-standalone layout).
#[cfg_attr(dev, allow(dead_code))]
fn bundled_py_rel() -> &'static str {
    if cfg!(windows) {
        "python.exe"
    } else {
        "bin/python3"
    }
}

/// Derived pyserver paths, resolved once in setup as global state (commands never re-resolve).
#[derive(Debug, Clone)]
pub struct PyPaths {
    /// Service code root: dev = repo pyserver/, prod = install resources/pyserver.
    pub root: PathBuf,
    /// stdlib-only env probe script (runs under either the bundled interpreter or a venv).
    pub bootstrap: PathBuf,
    /// Interpreter that runs pyserver: Windows prod = bundled Python; Linux prod = user venv (read-only install dir); dev = pyserver/.venv.
    pub python: PathBuf,
    /// Bundled base interpreter (read-only): Linux prod creates the venv with it; Windows prod equals python; dev = None.
    pub base_python: Option<PathBuf>,
    /// Venv root when a user-level venv is needed (Linux prod = ~/.ezpdf/venv; dev = pyserver/).
    pub venv_dir: Option<PathBuf>,
    /// Models dir, kept outside the code: prod = ~/.ezpdf/models (survives upgrades), dev = pyserver/models.
    pub models: PathBuf,
}

impl PyPaths {
    pub fn resolve(app: &AppHandle) -> Result<Self, String> {
        let (root, bundled) = server_root(app)?;
        let models = models_dir(app, &root);
        let bootstrap = root.join("bootstrap.py");
        // Runtime interpreter override (dev emulating prod / non-standard deploys): no venv.
        if let Ok(p) = std::env::var("EZPDF_PYTHON_EXE") {
            if !p.trim().is_empty() {
                let python = PathBuf::from(p);
                return Ok(Self {
                    bootstrap,
                    python: python.clone(),
                    base_python: Some(python),
                    venv_dir: None,
                    models,
                    root,
                });
            }
        }
        let bundled_py = bundled.then(|| bundled_python(app)).transpose()?;
        let venv_dir = venv_root(app, &root, bundled);
        let python = if let (true, Some(venv)) = (needs_own_venv(bundled), venv_dir.as_ref()) {
            venv_join(venv)
        } else {
            bundled_py
                .clone()
                .unwrap_or_else(|| venv_join(&root.join(".venv")))
        };
        Ok(Self {
            bootstrap,
            python,
            base_python: bundled_py,
            venv_dir,
            models,
            root,
        })
    }
}

/// Read-only install dirs (Linux/macOS: root-owned /usr/lib, read-only AppImage) need a user venv since pip can't write the bundled interpreter.
fn needs_own_venv(bundled: bool) -> bool {
    bundled && !cfg!(windows)
}

/// Interpreter path inside a venv (Windows: Scripts/python.exe; Unix: bin/python3).
fn venv_join(venv_dir: &Path) -> PathBuf {
    if cfg!(windows) {
        venv_dir.join("Scripts").join(py_exe_name())
    } else {
        venv_dir.join("bin").join(py_exe_name())
    }
}

/// User venv root: prod non-Windows = ~/.ezpdf/venv (beside models); Windows prod = None (writable install dir); dev = pyserver/.venv.
fn venv_root(app: &AppHandle, root: &Path, bundled: bool) -> Option<PathBuf> {
    if !bundled || cfg!(windows) {
        let _ = app;
        return Some(root.join(".venv"));
    }
    #[cfg(dev)]
    {
        let _ = app;
        Some(root.join(".venv"))
    }
    #[cfg(not(dev))]
    {
        app.path()
            .home_dir()
            .ok()
            .map(|home| home.join(".ezpdf").join("venv"))
    }
}

/// Bundled base interpreter (read-only; probes the same two locations as the pyserver root).
#[cfg_attr(dev, allow(dead_code))]
fn bundled_python(app: &AppHandle) -> Result<PathBuf, String> {
    #[cfg(dev)]
    {
        let _ = app;
        Err("no bundled interpreter under dev".into())
    }
    #[cfg(not(dev))]
    {
        let res = app.path().resource_dir().map_err(|e| e.to_string())?;
        let direct = res.join("python").join(bundled_py_rel());
        if direct.is_file() {
            return Ok(direct);
        }
        Ok(res.join("resources").join("python").join(bundled_py_rel()))
    }
}

/// Service code root: dev = repo pyserver/, prod = install resources (two-location probe); EZPDF_TOOLKIT_ROOT overrides.
fn server_root(app: &AppHandle) -> Result<(PathBuf, bool), String> {
    if let Ok(root) = std::env::var("EZPDF_TOOLKIT_ROOT") {
        if !root.trim().is_empty() {
            return Ok((PathBuf::from(root), false));
        }
    }
    #[cfg(dev)]
    {
        let _ = app; // dev branch doesn't need AppHandle
        Ok((PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../pyserver"), false))
    }
    #[cfg(not(dev))]
    {
        let res = app.path().resource_dir().map_err(|e| e.to_string())?;
        // bundle.resources lands at <resource root>/pyserver; also accept a nested resources/ for custom layouts.
        let direct = res.join("pyserver");
        if direct.join("bootstrap.py").is_file() {
            return Ok((direct, true));
        }
        Ok((res.join("resources").join("pyserver"), true))
    }
}

/// Models dir: EZPDF_MODELS_DIR overrides; prod ~/.ezpdf/models (1.9 GB survives upgrades); dev = pyserver/models.
fn models_dir(app: &AppHandle, root: &Path) -> PathBuf {
    if let Ok(dir) = std::env::var("EZPDF_MODELS_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    #[cfg(dev)]
    {
        let _ = app;
        root.join("models")
    }
    #[cfg(not(dev))]
    {
        match app.path().home_dir() {
            Ok(home) => home.join(".ezpdf").join("models"),
            Err(_) => root.join("models"),
        }
    }
}

#[cfg(windows)]
fn hide_window(cmd: &mut StdCommand) {
    cmd.creation_flags(CREATE_NO_WINDOW);
}
#[cfg(not(windows))]
fn hide_window(_cmd: &mut StdCommand) {}

// ---- bootstrap.py probe ----

/// bootstrap.py output (snake_case keys); parsed internally and composed into OcrEnvReport.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
struct BootstrapRaw {
    python: Option<String>,
    python_path: Option<String>,
    /// A crashing probe emits only {"error": ...}: default the rest so the reason survives.
    #[serde(default)]
    deps: BTreeMap<String, Option<String>>,
    #[serde(default)]
    missing: Vec<String>,
    torch_build: Option<String>,
    gpu: Option<BootstrapGpu>,
    models: Option<BootstrapModels>,
    error: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
struct BootstrapGpu {
    name: String,
    driver: String,
    cuda: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
struct BootstrapModels {
    layout: bool,
    vl: bool,
}

/// Layered env report (source for the OCR settings status lights); Rust owns it and pushes it whole.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct OcrEnvReport {
    pub python: Option<String>,
    pub python_path: Option<String>,
    pub deps: BTreeMap<String, Option<String>>,
    pub missing: Vec<String>,
    pub torch_build: Option<String>,
    pub gpu: Option<GpuInfo>,
    pub models: Option<OcrModels>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct GpuInfo {
    pub name: String,
    pub driver: String,
    pub cuda: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct OcrModels {
    pub layout: bool,
    pub vl: bool,
}

fn compose(raw: BootstrapRaw) -> OcrEnvReport {
    OcrEnvReport {
        python: raw.python,
        python_path: raw.python_path,
        deps: raw.deps,
        missing: raw.missing,
        torch_build: raw.torch_build,
        gpu: raw.gpu.map(|g| GpuInfo { name: g.name, driver: g.driver, cuda: g.cuda }),
        models: raw.models.map(|m| OcrModels { layout: m.layout, vl: m.vl }),
        error: raw.error,
    }
}

/// Empty report when probing fails; the error distinguishes "no interpreter" from the script's real failure.
fn failed_report(error: String) -> OcrEnvReport {
    OcrEnvReport {
        python: None,
        python_path: None,
        deps: BTreeMap::new(),
        missing: Vec::new(),
        torch_build: None,
        gpu: None,
        models: None,
        error: Some(error),
    }
}

/// Run bootstrap.py once (service interpreter first, system python in dev); a probe failure is data on the report's error field, not an Err.
fn probe_blocking(paths: &PyPaths) -> OcrEnvReport {
    // Prefer the service interpreter, then the bundled base: the report must describe the one we'll actually use.
    let python = std::iter::once(&paths.python)
        .chain(paths.base_python.iter())
        .find(|p| p.is_file())
        .cloned()
        .or_else(find_system_python);
    match python {
        Some(py) => match run_bootstrap(&py, &paths.bootstrap, &paths.models) {
            Ok(raw) => compose(raw),
            Err(e) => {
                println!("[pyenv] bootstrap failed: {e}");
                failed_report(format!("environment probe failed: {e}"))
            }
        },
        None => failed_report("no usable Python detected".into()),
    }
}

pub async fn probe(paths: &PyPaths) -> OcrEnvReport {
    let p = paths.clone();
    tauri::async_runtime::spawn_blocking(move || probe_blocking(&p))
        .await
        .unwrap_or_else(|e| failed_report(format!("environment probe task failed: {e}")))
}

fn run_bootstrap(python: &Path, script: &Path, models: &Path) -> Result<BootstrapRaw, String> {
    let mut cmd = StdCommand::new(python);
    cmd.arg(script).env("EZPDF_MODELS_DIR", models);
    hide_window(&mut cmd);
    let out = cmd
        .output()
        .map_err(|e| format!("failed to run bootstrap: {e}"))?;
    if !out.status.success() {
        return Err(format!("bootstrap exited with code {:?}", out.status.code()));
    }
    // Scan lines in reverse for parseable JSON (tolerates stray interpreter stdout).
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines().rev() {
        if let Ok(raw) = serde_json::from_str::<BootstrapRaw>(line.trim()) {
            return Ok(raw);
        }
    }
    Err("cannot parse bootstrap stdout".into())
}

/// System interpreter discovery: Windows `py -3` → `python`; Unix `python3` → `python` (Debian has no bare `python`).
fn find_system_python() -> Option<PathBuf> {
    if cfg!(windows) {
        try_python("py", &["-3"]).or_else(|| try_python("python", &[]))
    } else {
        try_python("python3", &[]).or_else(|| try_python("python", &[]))
    }
}

fn try_python(exe: &str, pre: &[&str]) -> Option<PathBuf> {
    let mut cmd = StdCommand::new(exe);
    cmd.args(pre).args(["-c", "import sys;print(sys.executable)"]);
    hide_window(&mut cmd);
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let path = text.lines().next()?.trim();
    let p = PathBuf::from(path);
    if p.is_file() { Some(p) } else { None }
}

// ---- Service install: explicit CPU/GPU choice + mirror switch + staged progress ----

/// Install mode (frontend radio; GPU only requires nvidia-smi, driver compatibility is runtime feedback).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    Cpu,
    Gpu,
}

impl InstallMode {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s {
            "cpu" => Ok(Self::Cpu),
            "gpu" => Ok(Self::Gpu),
            other => Err(format!("unknown install mode: {other} (cpu/gpu)")),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Gpu => "GPU",
        }
    }

    fn variant_file(self) -> &'static str {
        match self {
            Self::Cpu => "requirements-torch-cpu.txt",
            Self::Gpu => "requirements-torch-cuda.txt",
        }
    }
}

/// Install progress event (ocr://install): phase name + estimated percent.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct InstallProgress {
    pub phase: String,
    pub percent: u32,
}

async fn emit_log(app: &AppHandle, line: String) {
    let _ = app.emit("ocr://log", line);
}

async fn emit_progress(app: &AppHandle, phase: &str, percent: u32) {
    let _ = app.emit(
        "ocr://install",
        InstallProgress {
            phase: phase.to_string(),
            percent: percent.min(100),
        },
    );
}

/// Forward child output (both streams); every 3 lines interpolate progress across the phase range (pip's bar is off, so it's an estimate).
pub(crate) async fn forward_lines<R: tokio::io::AsyncRead + Unpin>(
    r: &mut R,
    app: &AppHandle,
    progress: Option<(&(String, u32, u32), &AtomicU32)>,
) {
    use tokio::io::AsyncBufReadExt;
    let mut buf = tokio::io::BufReader::new(r);
    let mut line = String::new();
    loop {
        line.clear();
        match buf.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let trimmed = line.trim_end();
                if trimmed.is_empty() {
                    continue;
                }
                emit_log(app, trimmed.to_string()).await;
                if let Some(((phase, from, to), counter)) = progress {
                    let n = counter.fetch_add(1, Ordering::Relaxed) + 1;
                    if n % 3 == 0 {
                        let step = (n / 3).min(to.saturating_sub(*from));
                        emit_progress(app, phase, from + step).await;
                    }
                }
            }
        }
    }
}

/// Run a child and stream its output (stdout/stderr merged into ocr://log; progress optional).
async fn run_streamed(
    app: &AppHandle,
    exe: &Path,
    args: &[String],
    cwd: &Path,
    models: &Path,
    envs: &[(&str, &str)],
    progress: Option<(String, u32, u32)>,
) -> Result<(), String> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(args)
        .current_dir(cwd)
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1")
        .env("EZPDF_MODELS_DIR", models)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
    let (mut out_r, mut err_r) = match (child.stdout.take(), child.stderr.take()) {
        (Some(o), Some(e)) => (o, e),
        _ => return Err("child process pipe missing".into()),
    };
    let counter = Arc::new(AtomicU32::new(0));
    let app_out = app.clone();
    let app_err = app.clone();
    let c_out = counter.clone();
    let c_err = counter.clone();
    let progress_out = progress.clone();
    let progress_err = progress;
    let t_out = tauri::async_runtime::spawn(async move {
        forward_lines(&mut out_r, &app_out, progress_out.as_ref().map(|p| (p, c_out.as_ref()))).await;
    });
    let t_err = tauri::async_runtime::spawn(async move {
        forward_lines(&mut err_r, &app_err, progress_err.as_ref().map(|p| (p, c_err.as_ref()))).await;
    });
    let status = child
        .wait()
        .await
        .map_err(|e| format!("failed to wait for child process: {e}"))?;
    let _ = t_out.await;
    let _ = t_err.await;
    if !status.success() {
        return Err(format!("command failed ({status})"));
    }
    Ok(())
}

fn svec(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

/// pip install: the mirror switch picks index args (the torch variant file's index-url was removed so sources can swap).
async fn pip_install(
    app: &AppHandle,
    paths: &PyPaths,
    req_file: &str,
    use_mirror: bool,
    progress: Option<(String, u32, u32)>,
) -> Result<(), String> {
    let file = paths.root.join(req_file);
    let mut args = svec(&[
        "-m",
        "pip",
        "install",
        "-r",
        &file.to_string_lossy(),
        "--progress-bar",
        "off",
        // Mirrors occasionally drop packages (IncompleteRead): widen retries and timeout.
        "--retries",
        "6",
        "--timeout",
        "60",
    ]);
    if req_file.contains("torch-cuda") {
        let index = if use_mirror {
            format!("{TORCH_MIRROR_BASE}/cu132")
        } else {
            format!("{TORCH_OFFICIAL_BASE}/cu132")
        };
        let extra = if use_mirror { PYPI_MIRROR } else { PYPI_OFFICIAL };
        args.extend(svec(&["--index-url", &index, "--extra-index-url", extra]));
    } else if req_file.contains("torch-cpu") && !cfg!(windows) {
        // PyPI's Linux torch silently bundles CUDA (no +cu tag, so version checks miss it); the CPU variant must use the /cpu index. Windows PyPI torch is already CPU-only.
        let index = if use_mirror {
            format!("{TORCH_MIRROR_BASE}/cpu")
        } else {
            format!("{TORCH_OFFICIAL_BASE}/cpu")
        };
        let extra = if use_mirror { PYPI_MIRROR } else { PYPI_OFFICIAL };
        args.extend(svec(&["--index-url", &index, "--extra-index-url", extra]));
    } else if use_mirror {
        // Windows CPU torch and the rest live on PyPI: mirror = TUNA (official needs no args).
        args.extend(svec(&["--index-url", PYPI_MIRROR]));
    }
    run_streamed(app, &paths.python, &args, &paths.root, &paths.models, &[], progress).await
}

/// A CPU env is a subset of a GPU env: a "cuda" build satisfies a CPU request, not vice versa.
fn env_satisfies(report: &OcrEnvReport, mode: InstallMode) -> bool {
    if report.python.is_none() || !report.missing.is_empty() {
        return false;
    }
    match report.torch_build.as_deref() {
        Some("cuda") => true,
        Some("cpu") => mode == InstallMode::Cpu,
        _ => false,
    }
}

/// Install: interpreter (dev creates a venv if missing) → GPU precheck → base deps → torch variant; returns true when nothing needed installing.
pub async fn install_env(
    app: &AppHandle,
    paths: &PyPaths,
    mode: InstallMode,
    use_mirror: bool,
) -> Result<bool, String> {
    // Probe first: a complete env with a satisfying torch build skips install entirely.
    let report = probe_blocking(paths);
    if env_satisfies(&report, mode) {
        emit_log(
            app,
            format!(
                "[ezpdf] service already installed (torch {} build satisfies {} selection), skipping install",
                report.torch_build.as_deref().unwrap_or("?"),
                mode.label()
            ),
        )
        .await;
        return Ok(true);
    }
    emit_log(
        app,
        format!(
            "[ezpdf] OCR service installation started ({} mode{})",
            mode.label(),
            if use_mirror { " with mirror" } else { "" }
        ),
    )
    .await;
    emit_progress(app, "preparing", 2).await;
    if !paths.python.is_file() {
        // Interpreter missing: Linux prod makes a venv with the bundled Python; dev uses system python.
        let base = std::iter::once(paths.base_python.clone())
            .chain(std::iter::once(find_system_python()))
            .flatten()
            .find(|p| p.is_file())
            .ok_or("no Python detected; cannot create environment (install Python 3.10+ first)")?;
        let venv_dir = paths.venv_dir.clone().ok_or("venv directory configuration missing")?;
        emit_log(app, format!("[ezpdf] creating venv ({})…", venv_dir.display())).await;
        run_streamed(
            app,
            &base,
            &svec(&["-m", "venv", &venv_dir.to_string_lossy()]),
            &paths.root,
            &paths.models,
            &[],
            None,
        )
        .await?;
        if !paths.python.is_file() {
            return Err("interpreter still missing after venv creation".into());
        }
    }
    if mode == InstallMode::Gpu {
        let report = probe_blocking(paths);
        let Some(gpu) = report.gpu else {
            return Err(
                "no NVIDIA GPU detected (nvidia-smi unavailable); installation stopped; update GPU drivers or switch to CPU mode"
                    .into(),
            );
        };
        let cuda = gpu.cuda.map(|c| format!(" / CUDA {c}")).unwrap_or_default();
        emit_log(
            app,
            format!("[ezpdf] GPU detected: {} / driver {}{}", gpu.name, gpu.driver, cuda),
        )
        .await;
    }
    emit_progress(app, "installing base dependencies", 5).await;
    pip_install(
        app,
        paths,
        "requirements.txt",
        use_mirror,
        Some(("installing base dependencies".to_string(), 5, 42)),
    )
    .await?;
    let report = probe_blocking(paths);
    if !env_satisfies(&report, mode) {
        emit_log(app, format!("[ezpdf] installing torch ({})…", mode.label())).await;
        pip_install(
            app,
            paths,
            mode.variant_file(),
            use_mirror,
            Some(("installing torch".to_string(), 45, 72)),
        )
        .await?;
    } else {
        emit_log(
            app,
            format!("[ezpdf] torch is already the {} build, skipping", report.torch_build.as_deref().unwrap_or("?")),
        )
        .await;
    }
    emit_progress(app, "environment ready", 75).await;
    emit_log(app, "[ezpdf] OCR environment installation complete".into()).await;
    Ok(false)
}

/// Model download: install huggingface_hub on demand → `python -m app.fetch` (snapshot, resumable). Mirror: hf-mirror.com vs huggingface.co.
pub async fn download_models(
    app: &AppHandle,
    paths: &PyPaths,
    use_mirror: bool,
) -> Result<(), String> {
    if !paths.python.is_file() {
        return Err("interpreter not found; install the service first".into());
    }
    emit_log(app, "[ezpdf] model download started (about 1.9GB, downloads only what's missing)".into()).await;
    emit_progress(app, "installing downloader", 76).await;
    pip_install(app, paths, "requirements-download.txt", use_mirror, None).await?;
    emit_progress(app, "downloading models", 80).await;
    let hf = if use_mirror {
        "https://hf-mirror.com"
    } else {
        "https://huggingface.co"
    };
    run_streamed(
        app,
        &paths.python,
        &svec(&["-m", "app.fetch"]),
        &paths.root,
        &paths.models,
        &[("HF_ENDPOINT", hf)],
        Some(("downloading models".to_string(), 80, 99)),
    )
    .await?;
    emit_progress(app, "done", 100).await;
    emit_log(app, "[ezpdf] model download complete".into()).await;
    Ok(())
}
