// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::Path;
use tauri::Manager;
use ts_rs::TS;

pub mod parse;
pub mod pyenv;
pub mod pyserver;

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
#[derive(Deserialize, Serialize, TS)]
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

/// 页：index 从 1 起（与绑定 JSON 一致）；finished 标记该页是否完成处理
#[derive(Clone, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    pub index: u32,
    pub finished: bool,
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

//Get repoTree from config
#[tauri::command]
async fn gettree_from_config(app: tauri::AppHandle, root: &str) -> Result<RepoTree, String> {
    // 渲染取数闸门（阶段2）：前端凭 asset protocol 读取仓库内 PDF 二进制。
    // 静态 scope 留空，此处按仓库根运行时放行（最小权限）——选仓库/刷新必经本命令；
    // AppHandle 由 Tauri 注入，前端 invoke 参数不变
    app.asset_protocol_scope()
        .allow_directory(root, true)
        .map_err(|e| format!("Failed when allowing asset scope: {e}"))?;
    read_index(root)
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
    let pdf_path = dir
        .join(format!("{name}-{}.pdf", entry.id))
        .to_string_lossy()
        .to_string();
    let (json_path, status, pages) = match &entry.bind {
        Some(rel) => {
            let json_abs = dir.join(rel).to_string_lossy().to_string();
            let text = fs::read_to_string(&json_abs)
                .map_err(|e| format!("Failed when reading bound JSON: {e}"))?;
            let doc: BindDoc = serde_json::from_str(&text)
                .map_err(|e| format!("Failed when parsing bound JSON: {e}"))?;
            (Some(json_abs), doc.status, doc.pages)
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
    let pages: Vec<PageInfo> = match page_count {
        Some(n) => (1..=n)
            .map(|i| PageInfo {
                index: i,
                finished: false,
                blocks: Vec::new(),
            })
            .collect(),
        None => Vec::new(),
    };
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
                    if !index.folders.iter().any(|f| f == b) {
                        index.folders.push(b.clone());
                    }
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
) -> Result<(), String> {
    pyenv::install_env(&app, &paths).await
}

#[tauri::command]
async fn ocr_download_models(
    app: tauri::AppHandle,
    paths: tauri::State<'_, pyenv::PyPaths>,
) -> Result<(), String> {
    pyenv::download_models(&app, &paths).await
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

// ---- OCR 解析回路（阶段4）：批量 OCR + 骨架补齐；映射/写回细节在 parse.rs ----

/// parse_append 的后端落点：一批（≤4 页、同书）页图 → 整批推理 → 一次原子写回
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

/// 打开书补骨架：pdfjs 实测页数回填 pages 为空的绑定 JSON（lopdf 解析失败书）
#[tauri::command]
async fn prefill_pages(root: &str, id: &str, total: u32) -> Result<PDFStatus, String> {
    parse::prefill_pages(root, id, total).await
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            check_and_build_repo,
            gettree_from_config,
            load_pdf,
            import_pdf,
            ocr_env_report,
            ocr_install_env,
            ocr_download_models,
            ocr_start,
            ocr_stop,
            parse_pdf,
            prefill_pages,
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
