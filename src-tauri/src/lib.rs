// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Component, Path, PathBuf};
use tauri::Manager;
use ts_rs::TS;

pub mod parse;
pub mod pyenv;
pub mod pyserver;
pub mod translate;
pub mod update;

#[derive(Deserialize, Serialize, TS)]
#[ts(export)]
pub struct RepoTree {
    pub folders: Vec<String>,
    pub pdfs: Vec<PDFStruct>,
}

/// 索引条目（.ezrepo 平铺索引的一行）：
/// id = 稳定唯一标识符（导入时生成，挪动/改名不变）；name = PDF 名；
/// bind = 结构 JSON 的仓库相对路径（None = 未解析）；belong = 所属顶层目录名（None = 根级）。
#[derive(Clone, Serialize, TS, Deserialize)]
#[ts(export)]
pub struct PDFStruct {
    pub id: String,
    pub name: String,
    pub bind: Option<String>,
    pub belong: Option<String>,
}

// ---- ezpdf PDF 实体域模型（绑定 JSON <name>-<id>.json 与 load_pdf 载荷同构；ts-rs 导出到 bindings/，前端 domain.ts re-export）----

/// 解析状态机（书级）：Pending 未开始处理 / Processing 处理中 / Finished 完成。
/// OCR 管线接入后维护后两态，当前导入即 Pending。
#[derive(Clone, Copy, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "PascalCase")]
pub enum PDFStatus {
    Pending,
    Processing,
    Finished,
}

/// 版面块：OCR 检出的一个区域及其原文/译文。
/// kind 为 PP-DocLayoutV3 标签（text/title/list/figure/figure_caption/table/formula/header/footer），
/// 保留 string 通道以兼容后续新增标签，故不用 enum。
#[derive(Clone, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    #[serde(rename = "type")]
    pub kind: String,
    /// 原文内容：正文纯文本 / 公式 $$..$$ / 表格 markdown；figure 块为空串
    pub content: String,
    /// [x1, y1, x2, y2] 左上→右下角点，单位 PDF 点
    pub loc: [f64; 4],
    /// 译文；figure/formula 块为 None（formula 原样渲染），未译为 None
    pub translation: Option<String>,
}

/// 页：index 从 1 起（与绑定 JSON 一致）；finished 标记该页是否完成 OCR，
/// translated 标记该页是否完成 LLM 翻译（旧 JSON 无此字段默认 false → 自动补翻）
#[derive(Clone, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    pub index: u32,
    pub finished: bool,
    #[serde(default)]
    pub translated: bool,
    pub blocks: Vec<Block>,
}

/// 绑定 JSON（<name>-<id>.json）的磁盘格式；load_pdf 凭它回填 status/pages
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BindDoc {
    pub status: PDFStatus,
    pub pages: Vec<PageInfo>,
}

/// 一份 PDF 的完整实体（身份字段 + 绑定 JSON 内容）。
/// id = 稳定唯一标识符（与 .ezrepo 条目一致，前端一切键都以它为准）；
/// bind = 绑定 JSON 的仓库相对路径，None = 未绑定（旧条目）；
/// json_path = 同一份 JSON 的绝对路径（load_pdf 运行时由 bind 解析，与 bind 同生同灭：bind None 时为 None）；
/// status/pages = 绑定 JSON 的内容（bind None 时为 Pending/空）。
#[derive(Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PDF {
    pub id: String,
    pub name: String,
    pub pdf_path: String,
    pub json_path: Option<String>,
    pub bind: Option<String>,
    pub status: PDFStatus,
    pub pages: Vec<PageInfo>,
}

/// 多文件导入结果：best-effort——成功条目与逐文件失败原因一并返回
#[derive(Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ImportFailure {
    pub path: String,
    pub reason: String,
}

#[derive(Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    pub imported: Vec<PDFStruct>,
    pub failed: Vec<ImportFailure>,
    /// 导入成功但页数解析失败（加密/损坏 PDF）：pages 为空骨架，通知前端
    pub warnings: Vec<ImportFailure>,
}
// Check repo path input availablity
fn is_dir_empty<P: AsRef<Path>>(path: P) -> io::Result<bool> {
    let mut entries = fs::read_dir(path)?;
    match entries.next().transpose()? {
        Some(_) => Ok(false),
        None => Ok(true),
    }
}

#[tauri::command]
fn check_and_build_repo(root: &str) -> Result<bool, String> {
    /*
    true => created a repo
    false => opened an exist repo
    Err => Error message to handle
     */
    let dir = Path::new(root);
    let _sign_path = Path::new(dir).join(".ezrepo");
    if !dir.is_dir() {
        return Err(format!("{} is not a valid directory path", root));
    }
    match is_dir_empty(dir) {
        Ok(true) => {
            fs::write(&_sign_path, r#"{"folders":[],"pdfs":[]}"#)
                .map_err(|e| format!("Failed when creating .ezrepo file:{e}"))?;
            Ok(true)
        }
        Ok(false) => {
            if _sign_path.is_file() {
                Ok(false)
            } else {
                Err(format!("Path is not a valid repo"))
            }
        }
        _ => Err(format!("Failed when impletting \"is_dir_empty()\"")),
    }
}

/// 读取仓库索引（.ezrepo）
fn read_index(root: &str) -> Result<RepoTree, String> {
    let sign_path = Path::new(root).join(".ezrepo");
    let text = fs::read_to_string(&sign_path)
        .map_err(|e| format!("Failed when reading .ezrepo files: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("Failed when reading string: {e}"))
}

/// 回写仓库索引（pretty JSON）
fn write_index(root: &str, index: &RepoTree) -> Result<(), String> {
    let text = serde_json::to_string_pretty(index)
        .map_err(|e| format!("Failed when serializing .ezrepo: {e}"))?;
    fs::write(Path::new(root).join(".ezrepo"), text)
        .map_err(|e| format!("Failed when writing .ezrepo: {e}"))
}

/// 仓库相对路径词法校验：非空、无绝对路径/盘符前缀/`..`（允许 `./`）。
/// 防手写 `.ezrepo` 的 name/bind 逃逸仓库根。
fn check_relative(rel: &str) -> Result<(), String> {
    let ok = !rel.is_empty()
        && Path::new(rel)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if ok {
        Ok(())
    } else {
        Err(format!("拒绝越界的仓库相对路径: {rel}"))
    }
}

/// 解析绑定 JSON 路径：词法校验 + canonicalize 后必须仍在仓库根内
/// （词法拦截 `../`；canonicalize 再拦截仓库内符号链接逃逸）
fn resolve_bind_path(root: &str, rel: &str) -> Result<PathBuf, String> {
    check_relative(rel)?;
    let root_canon = fs::canonicalize(root).map_err(|e| format!("仓库根路径无效: {e}"))?;
    let resolved = fs::canonicalize(root_canon.join(rel))
        .map_err(|e| format!("Failed when reading bound JSON: {e}"))?;
    if !resolved.starts_with(&root_canon) {
        return Err(format!("拒绝越界的仓库相对路径: {rel}"));
    }
    Ok(resolved)
}

//Get repoTree from config
#[tauri::command]
async fn gettree_from_config(app: tauri::AppHandle, root: &str) -> Result<RepoTree, String> {
    // 先确认是仓库（读得到 .ezrepo）再授权，避免对任意目录递归放行 asset 协议；
    // 静态 scope 留空，此处按仓库根运行时放行（最小权限）——选仓库/刷新必经本命令；
    // AppHandle 由 Tauri 注入，前端 invoke 参数不变
    let index = read_index(root)?;
    app.asset_protocol_scope()
        .allow_directory(root, true)
        .map_err(|e| format!("Failed when allowing asset scope: {e}"))?;
    Ok(index)
}

/// 读并解析绑定 JSON（load_pdf / parse.rs 的批量写回共用同一错误文案与语义）
pub(crate) fn read_bind_doc(abs: &Path) -> Result<BindDoc, String> {
    let text = fs::read_to_string(abs).map_err(|e| format!("Failed when reading bound JSON: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("Failed when parsing bound JSON: {e}"))
}

// Open a PDF by its stable id: look up the .ezrepo entry, resolve paths, read the bound JSON.
#[tauri::command]
async fn load_pdf(_root: &str, _id: &str) -> Result<PDF, String> {
    let dir = Path::new(_root);
    let entry = read_index(_root)?
        .pdfs
        .into_iter()
        .find(|p| p.id == _id)
        .ok_or_else(|| format!("PDF id not found in repo: {_id}"))?;

    // 物理命名（导入时生成）：name-id.pdf / name-id.json，天然免重名
    let name = entry.name.clone();
    let pdf_name = format!("{name}-{}.pdf", entry.id);
    check_relative(&pdf_name)?; // 手写 .ezrepo 可带 ../ 的 name/id，拼装后同样拒绝
    let pdf_path = dir.join(&pdf_name).to_string_lossy().to_string();
    let (json_path, status, pages) = match &entry.bind {
        Some(rel) => {
            let json_abs = resolve_bind_path(_root, rel)?;
            let doc = read_bind_doc(&json_abs)?;
            (
                Some(json_abs.to_string_lossy().to_string()),
                doc.status,
                doc.pages,
            )
        }
        None => (None, PDFStatus::Pending, Vec::new()),
    };

    Ok(PDF {
        id: entry.id,
        name,
        pdf_path,
        json_path,
        bind: entry.bind,
        status,
        pages,
    })
}

/// 内容哈希 → 12 位十六进制稳定 id。同内容同 id（重复导入直接拦截）；
/// DefaultHasher 跨版本算法可能变化，但 id 只需仓库内唯一，不受影响。
fn content_id(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())[..12].to_string()
}

/// 实测 PDF 页数（lopdf 解析页树，含 xref/对象流）；加密或损坏 → None（调用方回退空骨架）
fn pdf_page_count(bytes: &[u8]) -> Option<u32> {
    let doc = lopdf::Document::load_mem(bytes).ok()?;
    u32::try_from(doc.get_pages().len()).ok()
}

/// 单文件导入：校验 → 内容哈希生成 id → copy 入库（name-id 命名）→ 写绑定 JSON
/// （pages 按实测页数预填充骨架，解析失败回退空数组并给出原因）→ 返回 (索引条目, 页数警告)
fn import_one(
    dir: &Path,
    belong: Option<&str>,
    src_path: &str,
    index: &RepoTree,
) -> Result<(PDFStruct, Option<String>), String> {
    let src = Path::new(src_path);
    if !src.is_file() {
        return Err("文件不存在".into());
    }
    let is_pdf = src
        .extension()
        .map(|e| e.to_string_lossy().eq_ignore_ascii_case("pdf"))
        .unwrap_or(false);
    if !is_pdf {
        return Err("仅支持 PDF 文件".into());
    }
    let name = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.is_empty() || name.contains(['/', '\\', ':']) {
        return Err("文件名无效".into());
    }

    let bytes = fs::read(src).map_err(|e| format!("读取失败: {e}"))?;
    let id = content_id(&bytes);
    if index.pdfs.iter().any(|p| p.id == id) {
        return Err("内容与已导入的 PDF 重复".into());
    }

    // 物理命名 name-id：不同目录导入同名文件也不会互相覆盖
    let pdf_name = format!("{name}-{id}.pdf");
    let json_name = format!("{name}-{id}.json");
    fs::write(dir.join(&pdf_name), &bytes).map_err(|e| format!("入库失败: {e}"))?;
    let page_count = pdf_page_count(&bytes);
    let pages = page_count.map(parse::build_skeleton).unwrap_or_default();
    let doc = BindDoc {
        status: PDFStatus::Pending,
        pages,
    };
    let json_text = serde_json::to_string_pretty(&doc)
        .map_err(|e| format!("Failed when serializing bound JSON: {e}"))?;
    fs::write(dir.join(&json_name), json_text).map_err(|e| format!("写入绑定 JSON 失败: {e}"))?;

    let warning =
        page_count.is_none().then(|| "无法解析页数（可能加密或非标准 PDF），pages 预填充跳过".into());
    Ok((
        PDFStruct {
            id,
            name,
            bind: Some(json_name),
            belong: belong.map(|b| b.to_string()),
        },
        warning,
    ))
}

// Import PDFs (multi-file, best-effort): copy into the repo, create bound JSON
// skeletons, append .ezrepo entries; report successes and per-file failures.
#[tauri::command]
async fn import_pdf(
    root: &str,
    belong: Option<String>,
    paths: Vec<String>,
) -> Result<ImportOutcome, String> {
    let dir = Path::new(root);
    let mut index = read_index(root)?;
    let mut imported: Vec<PDFStruct> = Vec::new();
    let mut failed: Vec<ImportFailure> = Vec::new();
    let mut warnings: Vec<ImportFailure> = Vec::new();

    for src in &paths {
        match import_one(dir, belong.as_deref(), src, &index) {
            Ok((entry, page_warning)) => {
                if let Some(b) = &entry.belong {
                    ensure_folder(&mut index, b);
                }
                index.pdfs.push(entry.clone());
                imported.push(entry);
                if let Some(reason) = page_warning {
                    warnings.push(ImportFailure {
                        path: src.clone(),
                        reason,
                    });
                }
            }
            Err(reason) => failed.push(ImportFailure {
                path: src.clone(),
                reason,
            }),
        }
    }
    write_index(root, &index)?;
    Ok(ImportOutcome {
        imported,
        failed,
        warnings,
    })
}

// ---- 仓库条目增删改（用户 2026-09-14）：文件夹为逻辑分组（belong 标签，不落物理目录），
//      PDF/绑定 JSON 始终平铺在仓库根；建/移文件夹只改索引，删文件夹级联删除其中的文件 ----

/// 文件夹名（逻辑标签）词法校验：非空、无路径分隔符/盘符、非 `.`/`..`、长度上限
fn check_folder_name(name: &str) -> Result<(), String> {
    let bad = name.trim().is_empty()
        || name.contains(['/', '\\', ':'])
        || name == "."
        || name == ".."
        || name.chars().count() > 64;
    if bad {
        Err("文件夹名无效（不能为空、不能含路径分隔符，最长 64 字符）".into())
    } else {
        Ok(())
    }
}

/// 确保索引里有该文件夹（幂等；belong 是逻辑标签，无物理目录）
fn ensure_folder(index: &mut RepoTree, name: &str) {
    if !index.folders.iter().any(|f| f == name) {
        index.folders.push(name.to_string());
    }
}

/// 新建文件夹（仅索引；重名直接拒绝）
#[tauri::command]
fn create_folder(root: &str, name: String) -> Result<RepoTree, String> {
    let name = name.trim().to_string();
    check_folder_name(&name)?;
    let mut index = read_index(root)?;
    if index.folders.iter().any(|f| f == &name) {
        return Err(format!("同名文件夹已存在: {name}"));
    }
    index.folders.push(name);
    write_index(root, &index)?;
    Ok(index)
}

/// 删除一份 PDF 的库内文件（PDF + 绑定 JSON）：best-effort——索引条目为准，
/// bind 越界/文件缺失不致命（残留文件不影响使用）
fn remove_pdf_files(root: &str, entry: &PDFStruct) {
    let pdf_name = format!("{}-{}.pdf", entry.name, entry.id);
    if check_relative(&pdf_name).is_ok() {
        let _ = fs::remove_file(Path::new(root).join(&pdf_name));
    }
    if let Some(rel) = &entry.bind {
        if let Ok(abs) = resolve_bind_path(root, rel) {
            let _ = fs::remove_file(abs);
        }
    }
}

/// 删除文件夹：级联删除其中的全部 PDF（索引 + 库内文件）。用户拍板 2026-09-14：
/// 不再拒绝非空——确认框明示“一并删除”后由用户决定。与在途 OCR/翻译批次的竞态：
/// 批任务重读绑定 JSON 失败即安全中止，不会复活已删数据。
#[tauri::command]
fn delete_folder(root: &str, name: String) -> Result<RepoTree, String> {
    let mut index = read_index(root)?;
    let pos = index
        .folders
        .iter()
        .position(|f| f == &name)
        .ok_or_else(|| format!("文件夹不存在: {name}"))?;
    let victims: Vec<PDFStruct> = index
        .pdfs
        .iter()
        .filter(|p| p.belong.as_deref() == Some(name.as_str()))
        .cloned()
        .collect();
    index.pdfs.retain(|p| p.belong.as_deref() != Some(name.as_str()));
    for victim in &victims {
        remove_pdf_files(root, victim);
    }
    index.folders.remove(pos);
    write_index(root, &index)?;
    Ok(index)
}

/// 删除 PDF：摘除索引条目 + best-effort 删除库内 PDF 与绑定 JSON
#[tauri::command]
fn delete_pdf(root: &str, id: &str) -> Result<RepoTree, String> {
    let mut index = read_index(root)?;
    let pos = index
        .pdfs
        .iter()
        .position(|p| p.id == id)
        .ok_or_else(|| format!("PDF id not found in repo: {id}"))?;
    let entry = index.pdfs.remove(pos);
    remove_pdf_files(root, &entry);
    write_index(root, &index)?;
    Ok(index)
}

/// 移动 PDF：belong=Some 移入文件夹（不存在则自动建组），None 移出到根级
#[tauri::command]
fn move_pdf(root: &str, id: &str, belong: Option<String>) -> Result<RepoTree, String> {
    let mut index = read_index(root)?;
    let belong = belong.map(|b| b.trim().to_string()).filter(|b| !b.is_empty());
    if let Some(b) = &belong {
        check_folder_name(b)?;
        ensure_folder(&mut index, b);
    }
    let entry = index
        .pdfs
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("PDF id not found in repo: {id}"))?;
    entry.belong = belong;
    write_index(root, &index)?;
    Ok(index)
}

// ---- 应用设置持久化（用户 2026-09-14）：dev = 仓库根 config.json；生产 = 应用所在目录 config.json ----

/// 设置文件位置：与应用同目录（用户拍板 2026-09-14：所有设置都存应用目录的 cfg 文件）。
/// 生产 = exe 所在目录（NSIS per-user 安装，目录可写）；dev = 源码仓库根。
fn settings_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    #[cfg(dev)]
    {
        let _ = app;
        Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config.json"))
    }
    #[cfg(not(dev))]
    {
        let _ = app;
        let exe = std::env::current_exe().map_err(|e| format!("无法定位应用路径: {e}"))?;
        let dir = exe.parent().ok_or("无法解析应用目录")?;
        Ok(dir.join("config.json"))
    }
}

/// 读设置：文件不存在 → Ok(None)（前端用默认值，不打断启动）
#[tauri::command]
fn load_settings(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let path = settings_path(&app)?;
    match fs::read_to_string(&path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("读取设置失败 {}: {e}", path.display())),
    }
}

/// 写设置（JSON 校验 + 临时文件原子替换）：退出设置页时前端整份下发
#[tauri::command]
fn save_settings(app: tauri::AppHandle, json: String) -> Result<(), String> {
    serde_json::from_str::<serde_json::Value>(&json).map_err(|e| format!("设置 JSON 非法: {e}"))?;
    let path = settings_path(&app)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("创建设置目录失败: {e}"))?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| format!("写入设置失败: {e}"))?;
    fs::rename(&tmp, &path).map_err(|e| format!("替换设置文件失败: {e}"))?;
    Ok(())
}

// ---- OCR 服务（阶段2.5）：环境报告 / 环境安装 / 生命周期。探测与安装细节在 pyenv.rs，
//      进程状态机在 pyserver.rs；check_python/check_cuda 已被 ocr_env_report 取代 ----

#[tauri::command]
async fn ocr_env_report(paths: tauri::State<'_, pyenv::PyPaths>) -> Result<pyenv::OcrEnvReport, String> {
    Ok(pyenv::probe(&paths).await)
}

#[tauri::command]
async fn ocr_install_env(
    app: tauri::AppHandle,
    paths: tauri::State<'_, pyenv::PyPaths>,
    mode: String,
    use_mirror: bool,
) -> Result<(), String> {
    pyenv::install_env(&app, &paths, pyenv::InstallMode::parse(&mode)?, use_mirror).await
}

#[tauri::command]
async fn ocr_download_models(
    app: tauri::AppHandle,
    paths: tauri::State<'_, pyenv::PyPaths>,
    use_mirror: bool,
) -> Result<(), String> {
    pyenv::download_models(&app, &paths, use_mirror).await
}

#[tauri::command]
async fn ocr_start(
    app: tauri::AppHandle,
    paths: tauri::State<'_, pyenv::PyPaths>,
    svc: tauri::State<'_, pyserver::PyService>,
) -> Result<(), String> {
    if !svc.startable() {
        return Ok(()); // Starting/Connected 期间忽略重复拉起
    }
    svc.reset_for_start();
    let app2 = app.clone();
    let paths2 = paths.inner().clone();
    let svc2 = svc.inner().clone();
    tauri::async_runtime::spawn(pyserver::supervise(app2, paths2, svc2));
    Ok(())
}

#[tauri::command]
async fn ocr_stop(svc: tauri::State<'_, pyserver::PyService>) -> Result<(), String> {
    svc.stop(); // 关 stdin → Python stdin-EOF 自灭；状态经 ocr://status 事件回报
    Ok(())
}

// ---- OCR 解析回路（阶段4）：批量 OCR + 骨架补齐 + 同批 LLM 翻译；细节在 parse.rs / translate.rs ----

/// parse_append 的后端落点：一批（≤4 页、同书）页图 → 整批 OCR → 一次原子写回；
/// 翻译由前端随后调 translate_pdf 并发执行（用户拍板：pipeline 不再等翻译）
#[tauri::command]
async fn parse_pdf(
    root: &str,
    id: &str,
    pages: Vec<parse::ParsePageInput>,
    svc: tauri::State<'_, pyserver::PyService>,
) -> Result<parse::ParseOutcome, String> {
    let (base, token) = svc
        .ocr_target()
        .ok_or_else(|| "OCR 服务未连接".to_string())?;
    parse::parse_batch(root, id, pages, &base, &token).await
}

/// 翻译失败页重试（不 OCR）：finished && !translated 的页才处理
#[tauri::command]
async fn translate_pdf(
    root: &str,
    id: &str,
    pages: Vec<u32>,
    llm: translate::LlmConfig,
) -> Result<parse::ParseOutcome, String> {
    parse::translate_batch(root, id, pages, &llm).await
}

/// 打开书补骨架：pdfjs 实测页数回填 pages 为空的绑定 JSON（lopdf 解析失败书）
#[tauri::command]
async fn prefill_pages(root: &str, id: &str, total: u32) -> Result<PDFStatus, String> {
    parse::prefill_pages(root, id, total).await
}

/// LLM 连通性验证 + 关思考策略探测（设置页 API Key 旁「验证」按钮，用户 2026-09-14）
#[tauri::command]
async fn verify_llm(llm: translate::LlmConfig) -> Result<translate::LlmVerifyReport, String> {
    translate::verify_llm(&llm).await
}

/// 拉取 OpenAI 兼容 /models 列表（模型输入框自动补全，用户 2026-09-14）
#[tauri::command]
async fn fetch_llm_models(base_url: String, api_key: String) -> Result<Vec<String>, String> {
    translate::fetch_models(&base_url, &api_key).await
}

/// 检查更新（GitHub Releases 最新版本 vs 应用版本；启动静默/手动 toast 由前端决定）
#[tauri::command]
async fn check_update(app: tauri::AppHandle) -> Result<update::UpdateInfo, String> {
    let current = app.package_info().version.to_string();
    update::check_update(&current).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let paths = pyenv::PyPaths::resolve(app.handle())?;
            println!("[ezpdf] PyPaths = {paths:?}");
            app.manage(paths);
            app.manage(pyserver::PyService::new());
            translate::init_log(app.handle()); // 翻译日志 → llm://log（LLM 设置页底部）
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            check_and_build_repo,
            gettree_from_config,
            load_pdf,
            import_pdf,
            create_folder,
            delete_folder,
            delete_pdf,
            move_pdf,
            load_settings,
            save_settings,
            ocr_env_report,
            ocr_install_env,
            ocr_download_models,
            ocr_start,
            ocr_stop,
            parse_pdf,
            translate_pdf,
            prefill_pages,
            verify_llm,
            fetch_llm_models,
            check_update,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // 退出收尾：关 stdin → Python stdin-EOF 自灭（防孤儿）
                if let Some(svc) = app.try_state::<pyserver::PyService>() {
                    svc.stop();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_relative_accepts_contained_paths() {
        assert!(check_relative("foo.json").is_ok());
        assert!(check_relative("sub/foo.json").is_ok());
        assert!(check_relative(r"sub\foo.json").is_ok());
        assert!(check_relative("./foo.json").is_ok());
    }

    #[test]
    fn check_relative_rejects_escapes() {
        assert!(check_relative("").is_err());
        assert!(check_relative("..").is_err());
        assert!(check_relative("../foo.json").is_err());
        assert!(check_relative("sub/../../foo.json").is_err());
        assert!(check_relative("/abs/foo.json").is_err());
        assert!(check_relative(r"C:\evil\foo.json").is_err());
        assert!(check_relative(r"a-..\..\evil.pdf").is_err());
    }

    #[test]
    fn folder_name_validation() {
        assert!(check_folder_name("理论").is_ok());
        assert!(check_folder_name("a b-c_1").is_ok());
        assert!(check_folder_name("").is_err());
        assert!(check_folder_name("   ").is_err());
        assert!(check_folder_name("..").is_err());
        assert!(check_folder_name("../evil").is_err());
        assert!(check_folder_name(r"sub\dir").is_err());
        assert!(check_folder_name("C:dir").is_err());
        assert!(check_folder_name(&"x".repeat(65)).is_err());
    }

    #[test]
    fn folder_crud_round_trip() {
        let root = std::env::temp_dir().join(format!("ezpdf-folder-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let root_s = root.to_string_lossy().to_string();
        write_index(&root_s, &RepoTree { folders: vec![], pdfs: vec![] }).unwrap();

        assert!(create_folder(&root_s, "理论".into()).is_ok());
        assert!(create_folder(&root_s, " 理论 ".into()).is_err()); // trim 后重名
        let index = create_folder(&root_s, "实验".into()).unwrap();
        assert_eq!(index.folders, vec!["理论".to_string(), "实验".to_string()]);

        // 非空文件夹级联删除：索引条目 + 库内 PDF/绑定 JSON 一并删除（用户拍板 2026-09-14）
        fs::write(root.join("book-abc.pdf"), b"pdf").unwrap();
        fs::write(root.join("book-abc.json"), b"{}").unwrap();
        let with_pdf = RepoTree {
            folders: index.folders.clone(),
            pdfs: vec![PDFStruct {
                id: "abc".into(),
                name: "book".into(),
                bind: Some("book-abc.json".into()),
                belong: Some("理论".into()),
            }],
        };
        write_index(&root_s, &with_pdf).unwrap();
        let after = delete_folder(&root_s, "理论".into()).unwrap();
        assert_eq!(after.folders, vec!["实验".to_string()]);
        assert!(after.pdfs.is_empty());
        assert!(!root.join("book-abc.pdf").exists());
        assert!(!root.join("book-abc.json").exists());
        assert!(delete_folder(&root_s, "理论".into()).is_err()); // 已删

        // 移动 PDF：根级 → 文件夹（自动建组保留）→ 根级
        let single = RepoTree {
            folders: vec!["实验".into()],
            pdfs: vec![PDFStruct {
                id: "xy".into(),
                name: "second".into(),
                bind: None,
                belong: None,
            }],
        };
        write_index(&root_s, &single).unwrap();
        let moved = move_pdf(&root_s, "xy", Some("实验".into())).unwrap();
        assert_eq!(moved.pdfs[0].belong.as_deref(), Some("实验"));
        let moved = move_pdf(&root_s, "xy", None).unwrap();
        assert_eq!(moved.pdfs[0].belong, None);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn resolve_bind_path_keeps_reads_inside_root() {
        let root = std::env::temp_dir().join(format!("ezpdf-sec-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("ok.json"), "{}").unwrap();
        let root_s = root.to_string_lossy().to_string();

        assert!(resolve_bind_path(&root_s, "ok.json").is_ok());
        assert!(resolve_bind_path(&root_s, "../ok.json").is_err());
        assert!(resolve_bind_path(&root_s, "missing.json").is_err());

        let outside = root
            .parent()
            .unwrap()
            .join(format!("ezpdf-sec-out-{}.json", std::process::id()));
        fs::write(&outside, "{}").unwrap();
        assert!(resolve_bind_path(&root_s, outside.to_string_lossy().as_ref()).is_err());

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(&outside);
    }
}
