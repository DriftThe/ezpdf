use std::collections::BTreeMap;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use ts_rs::TS;
#[cfg(not(dev))]
use tauri::Manager; // 仅生产分支的 resource_dir()/home_dir() 需要

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 一键安装服务的源（用户 2026-09-14：按钮旁「使用镜像源」checkbox 控制）：
/// - pypi 镜像 = 清华 TUNA；torch 镜像 = 上交 SJTU pytorch-wheels（含 cu132 全部 wheels）
/// - 官方源 = pypi.org / download.pytorch.org
const PYPI_MIRROR: &str = "https://pypi.tuna.tsinghua.edu.cn/simple";
const PYPI_OFFICIAL: &str = "https://pypi.org/simple";
const TORCH_MIRROR_BASE: &str = "https://mirror.sjtu.edu.cn/pytorch-wheels";
const TORCH_OFFICIAL_BASE: &str = "https://download.pytorch.org/whl";

#[cfg(windows)]
const PY_EXE: &str = "python.exe";
#[cfg(not(windows))]
const PY_EXE: &str = "python";

/// pyserver 全部派生路径。`run()` 的 setup 钩子启动即解析一次并存为全局状态，
/// 命令侧通过 `tauri::State<PyPaths>` 取用，不做二次解析。
#[derive(Debug, Clone)]
pub struct PyPaths {
    /// 服务代码根：dev = 仓库 pyserver/，生产 = 安装目录 resources/pyserver
    pub root: PathBuf,
    /// stdlib 环境探测脚本（纯标准库，随包解释器/venv 均可运行）
    pub bootstrap: PathBuf,
    pub requirements: PathBuf,
    /// 服务解释器：生产 = 随包可重定位 Python（安装目录 resources/python）；
    /// dev = 仓库 pyserver/.venv 内解释器（缺失时用系统 python 创建）
    pub python: PathBuf,
    /// 模型目录（与代码分离）：生产 = ~/.ezpdf/models（升级/重装不丢），dev = pyserver/models
    pub models: PathBuf,
    /// true = 随包解释器（生产）；false = venv（dev，需自建）
    pub bundled: bool,
}

impl PyPaths {
    pub fn resolve(app: &AppHandle) -> Result<Self, String> {
        let (root, bundled) = server_root(app)?;
        Ok(Self {
            bootstrap: root.join("bootstrap.py"),
            requirements: root.join("requirements.txt"),
            python: python_exe(app, &root, bundled)?,
            models: models_dir(app, &root),
            root,
            bundled,
        })
    }
}

/// 服务代码根：dev = 仓库内 pyserver/（编译期 CARGO_MANIFEST_DIR）；
/// 生产 = 安装目录资源（Tauri bundle.resources 落位，双位置探测）。
/// dev/prod 判定用 tauri-build 注入的 `cfg(dev)`；EZPDF_TOOLKIT_ROOT 运行期强制覆盖。
fn server_root(app: &AppHandle) -> Result<(PathBuf, bool), String> {
    if let Ok(root) = std::env::var("EZPDF_TOOLKIT_ROOT") {
        if !root.trim().is_empty() {
            return Ok((PathBuf::from(root), false));
        }
    }
    #[cfg(dev)]
    {
        let _ = app; // dev 分支用不到 AppHandle
        Ok((PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../pyserver"), false))
    }
    #[cfg(not(dev))]
    {
        let res = app.path().resource_dir().map_err(|e| e.to_string())?;
        // bundle.resources 映射落位：优先 <资源根>/resources/pyserver，兼容直接落位
        let nested = res.join("resources").join("pyserver");
        if nested.join("bootstrap.py").is_file() {
            return Ok((nested, true));
        }
        Ok((res.join("pyserver"), true))
    }
}

/// 解释器：EZPDF_PYTHON_EXE 运行期覆盖（dev 里模拟生产布局用）；
/// 生产 = 随包 Python（resources/python）；dev = <root>/.venv。
fn python_exe(app: &AppHandle, root: &Path, bundled: bool) -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("EZPDF_PYTHON_EXE") {
        if !p.trim().is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    if bundled {
        #[cfg(not(dev))]
        {
            let res = app.path().resource_dir().map_err(|e| e.to_string())?;
            let nested = res.join("resources").join("python").join(PY_EXE);
            if nested.is_file() {
                return Ok(nested);
            }
            return Ok(res.join("python").join(PY_EXE));
        }
        #[cfg(dev)]
        {
            let _ = app;
        }
    }
    Ok(venv_python(root))
}

/// venv 解释器位置：Windows 与 Unix 目录布局不同
fn venv_python(root: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        root.join(".venv").join("Scripts").join(PY_EXE)
    }
    #[cfg(not(windows))]
    {
        root.join(".venv").join("bin").join(PY_EXE)
    }
}

/// 模型目录：EZPDF_MODELS_DIR 运行期覆盖；生产固定用户目录 ~/.ezpdf/models
/// （1.9GB 下载与代码/安装目录分离，升级重装不丢），dev = pyserver/models。
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

/// 跑一次 bootstrap.py（服务解释器优先；不存在时系统 python 兜底——仅 dev 会走到）。
/// 探测自身失败不作为错误抛出——报告即数据（error 字段），前端据此显示引导。
pub fn probe_blocking(paths: &PyPaths) -> OcrEnvReport {
    let python = if paths.python.is_file() {
        Some(paths.python.clone())
    } else {
        find_system_python()
    };
    python
        .and_then(|py| run_bootstrap(&py, &paths.bootstrap, &paths.models).ok())
        .map(compose)
        .unwrap_or_else(no_python_report)
}

pub async fn probe(paths: &PyPaths) -> OcrEnvReport {
    let p = paths.clone();
    tauri::async_runtime::spawn_blocking(move || probe_blocking(&p))
        .await
        .unwrap_or_else(|_| no_python_report())
}

fn run_bootstrap(python: &Path, script: &Path, models: &Path) -> Result<BootstrapRaw, String> {
    let mut cmd = StdCommand::new(python);
    cmd.arg(script).env("EZPDF_MODELS_DIR", models);
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

// ---- 一键安装服务（用户 2026-09-14）：CPU/GPU 显式选择 + 镜像开关 + 阶段进度 ------------------

/// 安装模式（前端 radio；GPU 只要求 nvidia-smi 存在，驱动兼容性由运行期反馈）
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
            other => Err(format!("未知安装模式: {other}（cpu/gpu）")),
        }
    }

    /// torch 构建标识（与 bootstrap 的 torch_build 口径一致）
    fn torch_build(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Gpu => "cuda",
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

/// 安装进度事件（ocr://install）：阶段名 + 估算百分比（按钮旁进度条）
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

/// 子进程输出逐行转发：每 3 行按阶段区间插值推一次进度（百分比只能估算——pip 进度条已关）
async fn forward_lines_counted<R: tokio::io::AsyncRead + Unpin>(
    r: &mut R,
    app: &AppHandle,
    progress: &Option<(String, u32, u32)>,
    counter: &AtomicU32,
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
                if let Some((phase, from, to)) = progress {
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

/// 跑子进程并流式转发输出（stdout/stderr 合并进 ocr://log；进度可选）
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
    let mut child = cmd.spawn().map_err(|e| format!("spawn 失败: {e}"))?;
    let (mut out_r, mut err_r) = match (child.stdout.take(), child.stderr.take()) {
        (Some(o), Some(e)) => (o, e),
        _ => return Err("子进程管道缺失".into()),
    };
    let counter = Arc::new(AtomicU32::new(0));
    let app_out = app.clone();
    let app_err = app.clone();
    let c_out = counter.clone();
    let c_err = counter.clone();
    let progress_out = progress.clone();
    let progress_err = progress;
    let t_out = tauri::async_runtime::spawn(async move {
        forward_lines_counted(&mut out_r, &app_out, &progress_out, &c_out).await;
    });
    let t_err = tauri::async_runtime::spawn(async move {
        forward_lines_counted(&mut err_r, &app_err, &progress_err, &c_err).await;
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

pub(crate) async fn forward_lines<R: tokio::io::AsyncRead + Unpin>(r: &mut R, app: &AppHandle) {
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

fn svec(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

/// pip 安装：镜像开关 → index 参数（torch 变体文件的 index-url 已在文件里移除，便于切换源）
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
        // 镜像源偶发 IncompleteRead（实测 TUNA 丢过包）：放宽重试与超时
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
    } else if use_mirror {
        // CPU torch 与其余依赖都在 PyPI：镜像 = TUNA（官方 = 默认 PyPI，无需参数）
        args.extend(svec(&["--index-url", PYPI_MIRROR]));
    }
    run_streamed(app, &paths.python, &args, &paths.root, &paths.models, &[], progress).await
}

/// 服务安装：解释器（dev 缺 venv 时用系统 python 创建）→ GPU 预检 → 基础依赖 →
/// torch 变体（按选择；已是目标构建则跳过）→ 完成。进度经 ocr://install 推送。
pub async fn install_env(
    app: &AppHandle,
    paths: &PyPaths,
    mode: InstallMode,
    use_mirror: bool,
) -> Result<(), String> {
    emit_log(
        app,
        format!(
            "[ezpdf] OCR 服务安装开始（{} 模式{}）",
            mode.label(),
            if use_mirror { "，使用镜像源" } else { "" }
        ),
    )
    .await;
    emit_progress(app, "准备", 2).await;
    if !paths.python.is_file() {
        // 生产随包解释器必在；走到这里只可能是 dev（venv 未建）
        let sys = find_system_python()
            .ok_or("未检测到 Python，无法创建环境（请先安装 Python 3.10+）")?;
        emit_log(app, "[ezpdf] 创建 venv…".into()).await;
        run_streamed(
            app,
            &sys,
            &svec(&["-m", "venv", ".venv"]),
            &paths.root,
            &paths.models,
            &[],
            None,
        )
        .await?;
        if !paths.python.is_file() {
            return Err("venv 创建后解释器仍不存在".into());
        }
    }
    if mode == InstallMode::Gpu {
        let report = probe_blocking(paths);
        let Some(gpu) = report.gpu else {
            return Err(
                "未检测到 NVIDIA GPU（nvidia-smi 不可用），已停止安装；请更新显卡驱动或改用 CPU 模式"
                    .into(),
            );
        };
        let cuda = gpu.cuda.map(|c| format!(" / CUDA {c}")).unwrap_or_default();
        emit_log(
            app,
            format!("[ezpdf] 检测到 GPU: {} / 驱动 {}{}", gpu.name, gpu.driver, cuda),
        )
        .await;
    }
    emit_progress(app, "安装基础依赖", 5).await;
    pip_install(
        app,
        paths,
        "requirements.txt",
        use_mirror,
        Some(("安装基础依赖".to_string(), 5, 42)),
    )
    .await?;
    if probe_blocking(paths).torch_build.as_deref() != Some(mode.torch_build()) {
        emit_log(app, format!("[ezpdf] 安装 torch（{}）…", mode.label())).await;
        pip_install(
            app,
            paths,
            mode.variant_file(),
            use_mirror,
            Some(("安装 torch".to_string(), 45, 72)),
        )
        .await?;
    } else {
        emit_log(app, format!("[ezpdf] torch 已是 {} 构建，跳过", mode.torch_build())).await;
    }
    emit_progress(app, "环境就绪", 75).await;
    emit_log(app, "[ezpdf] OCR 环境安装完成".into()).await;
    Ok(())
}

/// 模型下载：huggingface_hub（requirements-download.txt 按需补装，已装则 pip 秒过）
/// → python -m app.fetch（快照下载到模型目录；断点续传由 hub 库内置）。
/// 镜像开关：开 = hf-mirror（fetch.py 默认），关 = 官方 huggingface.co。
pub async fn download_models(
    app: &AppHandle,
    paths: &PyPaths,
    use_mirror: bool,
) -> Result<(), String> {
    if !paths.python.is_file() {
        return Err("解释器不存在，请先安装服务".into());
    }
    emit_log(app, "[ezpdf] 模型下载开始（约 1.9GB，缺哪补哪）".into()).await;
    emit_progress(app, "补装下载器", 76).await;
    pip_install(app, paths, "requirements-download.txt", use_mirror, None).await?;
    emit_progress(app, "下载模型", 80).await;
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
        Some(("下载模型".to_string(), 80, 99)),
    )
    .await?;
    emit_progress(app, "完成", 100).await;
    emit_log(app, "[ezpdf] 模型下载完成".into()).await;
    Ok(())
}
