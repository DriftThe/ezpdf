use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use tauri::AppHandle;
#[cfg(not(dev))]
use tauri::Manager; // 仅生产分支的 app.path().home_dir() 需要

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

#[cfg(windows)]
pub fn _is_python(executable: &Path) -> bool {
    let mut cmd = std::process::Command::new(executable);
    cmd.arg("--version");
    #[cfg(windows)]
    cmd.creation_flags(0x08000000);
    let out = match cmd.output() {
        // spawn 失败（解释器不存在/无权限）= 不可用
        Ok(out) => out,
        Err(_) => return false,
    };
    out.status.success()
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
