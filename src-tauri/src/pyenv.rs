use std::collections::BTreeMap;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use ts_rs::TS;
#[cfg(not(dev))]
use tauri::Manager; // 仅生产分支的 app.path().home_dir() 需要

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// pyserver 全部派生路径。`run()` 的 setup 钩子启动即解析一次并存为全局状态，
/// 命令侧通过 `tauri::State<PyPaths>` 取用，不做二次解析。
#[derive(Debug, Clone)]
pub struct PyPaths {
    /// 服务根目录：dev = 仓库 pyserver/，生产 = ~/.ezpdf/toolkit/server
    pub root: PathBuf,
    /// stdlib 环境探测脚本（纯标准库，venv 内外均可运行）
    pub bootstrap: PathBuf,
    pub requirements: PathBuf,
    /// 服务专用解释器（venv 内，探测/安装/启动服务都用它）
    pub venv_python: PathBuf,
    pub logs: PathBuf,
    /// 模型目录（阶段 3 定义目录约定后填充检测逻辑）
    pub models: PathBuf,
}

impl PyPaths {
    pub fn resolve(app: &AppHandle) -> Result<Self, String> {
        let root = server_root(app)?;
        Ok(Self {
            bootstrap: root.join("bootstrap.py"),
            requirements: root.join("requirements.txt"),
            venv_python: venv_python(&root),
            logs: root.join("logs"),
            models: root.join("models"),
            root,
        })
    }
}

/// dev = 仓库内 pyserver/（编译期 CARGO_MANIFEST_DIR）；
/// 生产 = ~/.ezpdf/toolkit/server。
/// dev/prod 判定用 tauri-build 注入的 `cfg(dev)`（编译期，等价于 is_dev 运行时判定）；
/// EZPDF_TOOLKIT_ROOT 非空时在运行期强制覆盖——dev 构建也能验证生产分支的解包/升级逻辑。
fn server_root(app: &AppHandle) -> Result<PathBuf, String> {
    if let Ok(root) = std::env::var("EZPDF_TOOLKIT_ROOT") {
        if !root.trim().is_empty() {
            return Ok(PathBuf::from(root));
        }
    }
    #[cfg(dev)]
    {
        let _ = app; // dev 分支用不到 AppHandle
        Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../pyserver"))
    }
    #[cfg(not(dev))]
    {
        let home = app.path().home_dir().map_err(|e| e.to_string())?;
        Ok(home.join(".ezpdf").join("toolkit").join("server"))
    }
}

/// venv 解释器位置：Windows 与 Unix 目录布局不同
fn venv_python(root: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        root.join(".venv").join("Scripts").join("python.exe")
    }
    #[cfg(not(windows))]
    {
        root.join(".venv").join("bin").join("python")
    }
}

#[cfg(windows)]
fn hide_window(cmd: &mut StdCommand) {
    cmd.creation_flags(CREATE_NO_WINDOW);
}
#[cfg(not(windows))]
fn hide_window(_cmd: &mut StdCommand) {}

// ---- bootstrap.py 探测 -------------------------------------------------------------------

/// bootstrap.py 输出（snake_case 键）；仅解析用，对外合成 OcrEnvReport
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
struct BootstrapRaw {
    python: Option<String>,
    python_path: Option<String>,
    in_venv: bool,
    deps: BTreeMap<String, Option<String>>,
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

/// 环境分层报告（OCR 设置页状态灯数据源）；Rust 持有并整份推给前端缓存渲染
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct OcrEnvReport {
    pub python: Option<String>,
    pub python_path: Option<String>,
    pub in_venv: bool,
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
        in_venv: raw.in_venv,
        deps: raw.deps,
        missing: raw.missing,
        torch_build: raw.torch_build,
        gpu: raw.gpu.map(|g| GpuInfo { name: g.name, driver: g.driver, cuda: g.cuda }),
        models: raw.models.map(|m| OcrModels { layout: m.layout, vl: m.vl }),
        error: raw.error,
    }
}

fn no_python_report() -> OcrEnvReport {
    OcrEnvReport {
        python: None,
        python_path: None,
        in_venv: false,
        deps: BTreeMap::new(),
        missing: Vec::new(),
        torch_build: None,
        gpu: None,
        models: None,
        error: Some("未检测到可用的 Python".into()),
    }
}

/// 跑一次 bootstrap.py（venv python 优先；否则系统 python 兜底）。
/// 探测自身失败不作为错误抛出——报告即数据（error 字段），前端据此显示引导。
pub fn probe_blocking(paths: &PyPaths) -> OcrEnvReport {
    if paths.venv_python.is_file() {
        return match run_bootstrap(&paths.venv_python, &paths.bootstrap) {
            Ok(raw) => compose(raw),
            Err(_) => no_python_report(),
        };
    }
    if let Some(py) = find_system_python() {
        return match run_bootstrap(&py, &paths.bootstrap) {
            Ok(raw) => compose(raw),
            Err(_) => no_python_report(),
        };
    }
    no_python_report()
}

pub async fn probe(paths: &PyPaths) -> OcrEnvReport {
    let p = paths.clone();
    tauri::async_runtime::spawn_blocking(move || probe_blocking(&p))
        .await
        .unwrap_or_else(|_| no_python_report())
}

fn run_bootstrap(python: &Path, script: &Path) -> Result<BootstrapRaw, String> {
    let mut cmd = StdCommand::new(python);
    cmd.arg(script);
    hide_window(&mut cmd);
    let out = cmd
        .output()
        .map_err(|e| format!("bootstrap 运行失败: {e}"))?;
    if !out.status.success() {
        return Err(format!("bootstrap 退出码 {:?}", out.status.code()));
    }
    // 逐行倒序找能解析成 JSON 的行（容忍解释器偶发的其他 stdout 输出）
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines().rev() {
        if let Ok(raw) = serde_json::from_str::<BootstrapRaw>(line.trim()) {
            return Ok(raw);
        }
    }
    Err("bootstrap stdout 无法解析".into())
}

/// 系统解释器发现：py -3 → PATH python
fn find_system_python() -> Option<PathBuf> {
    try_python("py", &["-3"]).or_else(|| try_python("python", &[]))
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

/// torch 安装变体选择：驱动 CUDA 主版本 ≥ 13 → cu132 wheels，否则 CPU wheels
/// （torch 2.13+cu132 依赖 CUDA 13.x minor-version 兼容驱动）
fn torch_variant(report: &OcrEnvReport) -> &'static str {
    let cuda_major = report
        .gpu
        .as_ref()
        .and_then(|g| g.cuda.as_deref())
        .and_then(|c| c.split('.').next())
        .and_then(|m| m.parse::<u32>().ok());
    if cuda_major.unwrap_or(0) >= 13 {
        "requirements-torch-cuda.txt"
    } else {
        "requirements-torch-cpu.txt"
    }
}

// ---- 环境安装（venv 创建 + pip，stderr/stdout 逐行推 ocr://log）---------------------------

async fn emit_log(app: &AppHandle, line: String) {
    let _ = app.emit("ocr://log", line);
}

async fn run_streamed(app: &AppHandle, exe: &Path, args: &[&str], cwd: &Path) -> Result<(), String> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUTF8", "1");
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let mut child = cmd.spawn().map_err(|e| format!("spawn 失败: {e}"))?;
    let (mut out_r, mut err_r) = match (child.stdout.take(), child.stderr.take()) {
        (Some(o), Some(e)) => (o, e),
        _ => return Err("子进程管道缺失".into()),
    };
    let app_out = app.clone();
    let app_err = app.clone();
    let t_out = tauri::async_runtime::spawn(async move {
        forward_lines(&mut out_r, &app_out).await;
    });
    let t_err = tauri::async_runtime::spawn(async move {
        forward_lines(&mut err_r, &app_err).await;
    });
    let status = child
        .wait()
        .await
        .map_err(|e| format!("子进程等待失败: {e}"))?;
    let _ = t_out.await;
    let _ = t_err.await;
    if !status.success() {
        return Err(format!("命令失败（{}）", status));
    }
    Ok(())
}

async fn forward_lines<R: tokio::io::AsyncRead + Unpin>(r: &mut R, app: &AppHandle) {
    use tokio::io::AsyncBufReadExt;
    let mut buf = tokio::io::BufReader::new(r);
    let mut line = String::new();
    loop {
        line.clear();
        match buf.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let trimmed = line.trim_end();
                if !trimmed.is_empty() {
                    emit_log(app, trimmed.to_string()).await;
                }
            }
        }
    }
}

async fn pip_install(app: &AppHandle, paths: &PyPaths, req_file: &str) -> Result<(), String> {
    let file = paths.root.join(req_file);
    let file_str = file.to_string_lossy().into_owned();
    run_streamed(
        app,
        &paths.venv_python,
        &["-m", "pip", "install", "-r", file_str.as_str(), "--progress-bar", "off"],
        &paths.root,
    )
    .await
}

/// 环境引导：venv 缺失则用系统 python 创建 → 基础依赖 → 按 GPU 探测装 torch 变体
pub async fn install_env(app: &AppHandle, paths: &PyPaths) -> Result<(), String> {
    emit_log(app, "[ezpdf] OCR 环境安装开始".into()).await;
    if !paths.venv_python.is_file() {
        let sys = find_system_python()
            .ok_or("未检测到 Python，无法创建环境（请先安装 Python 3.10+）")?;
        emit_log(app, "[ezpdf] 创建 venv…".into()).await;
        run_streamed(app, &sys, &["-m", "venv", ".venv"], &paths.root).await?;
        if !paths.venv_python.is_file() {
            return Err("venv 创建后解释器仍不存在".into());
        }
    }
    pip_install(app, paths, "requirements.txt").await?;
    let report = probe_blocking(paths);
    if report.torch_build.is_none() {
        let variant = torch_variant(&report);
        emit_log(app, format!("[ezpdf] 安装 torch 变体: {variant}")).await;
        pip_install(app, paths, variant).await?;
    }
    emit_log(app, "[ezpdf] OCR 环境安装完成".into()).await;
    Ok(())
}

/// 模型下载：huggingface_hub（requirements-download.txt 按需补装，已装则 pip 秒过）
/// → python -m app.fetch（快照下载到 models/，走 hf-mirror 镜像；断点续传由 hub 库内置）。
pub async fn download_models(app: &AppHandle, paths: &PyPaths) -> Result<(), String> {
    if !paths.venv_python.is_file() {
        return Err("venv 解释器不存在，请先安装环境".into());
    }
    emit_log(app, "[ezpdf] 模型下载开始（约 1.9GB，缺哪补哪）".into()).await;
    pip_install(app, paths, "requirements-download.txt").await?;
    run_streamed(app, &paths.venv_python, &["-m", "app.fetch"], &paths.root).await?;
    emit_log(app, "[ezpdf] 模型下载完成".into()).await;
    Ok(())
}
