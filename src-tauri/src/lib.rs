use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Component, Path, PathBuf};
use tauri::Manager;
use tauri::Emitter;
use ts_rs::TS;

pub mod parse;
pub mod pyenv;
pub mod pyserver;
pub mod table;
pub mod translate;
pub mod update;

#[derive(Deserialize, Serialize, TS)]
#[ts(export)]
pub struct RepoTree {
    pub folders: Vec<String>,
    pub pdfs: Vec<PDFStruct>,
}

/// One .ezrepo index line; id is minted at import; bind is the repo-relative bound-JSON path (None = unparsed); belong is a logical folder label (None = root).
#[derive(Clone, Serialize, TS, Deserialize)]
#[ts(export)]
pub struct PDFStruct {
    pub id: String,
    pub name: String,
    pub bind: Option<String>,
    pub belong: Option<String>,
}

// ---- Domain model: bound JSON and load_pdf payload are isomorphic; ts-rs bindings are re-exported by domain.ts ----

/// Book-level parse state; import creates Pending, the OCR pipeline drives the other two.
#[derive(Clone, Copy, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "PascalCase")]
pub enum PDFStatus {
    Pending,
    Processing,
    Finished,
}

/// One OCR-detected region; kind is a PP-DocLayoutV3 label kept as a string so new labels pass through.
#[derive(Clone, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    #[serde(rename = "type")]
    pub kind: String,
    /// Source content: plain text, $$..$$ formulas, or table markdown; empty for figures.
    pub content: String,
    /// [x1, y1, x2, y2] top-left → bottom-right, in PDF points.
    pub loc: [f64; 4],
    /// Translation; None = render content as-is. Table blocks hold the translated matrix as 2-D JSON (see table.rs).
    pub translation: Option<String>,
    /// Table grid (type == "table"; Rust parses+persists, frontend renders): None = unparsable/not backfilled → no cover.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grid: Option<table::TableGrid>,
}

/// 1-based page; finished = OCR done, translated = LLM pass done (older JSONs default false → re-translated).
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

/// On-disk bound-JSON format; load_pdf reads it back into status/pages.
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BindDoc {
    pub status: PDFStatus,
    pub pages: Vec<PageInfo>,
}

/// PDF entity; bind = repo-relative bound-JSON path (None = legacy/unbound → Pending, no pages).
#[derive(Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PDF {
    pub id: String,
    pub name: String,
    pub pdf_path: String,
    pub bind: Option<String>,
    pub status: PDFStatus,
    pub pages: Vec<PageInfo>,
}

/// Multi-file import result: best-effort — successes plus per-file failure reasons.
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
    /// Imported but page count unreadable (encrypted/damaged): pages is an empty skeleton.
    pub warnings: Vec<ImportFailure>,
}
fn is_dir_empty<P: AsRef<Path>>(path: P) -> io::Result<bool> {
    let mut entries = fs::read_dir(path)?;
    match entries.next().transpose()? {
        Some(_) => Ok(false),
        None => Ok(true),
    }
}

#[tauri::command]
fn check_and_build_repo(root: &str) -> Result<bool, String> {
    let dir = Path::new(root);
    if !dir.is_dir() {
        return Err(format!("{} is not a valid directory path", root));
    }
    let sign_path = dir.join(".ezrepo");
    match is_dir_empty(dir) {
        Ok(true) => {
            write_text_atomic(&sign_path, r#"{"folders":[],"pdfs":[]}"#)?;
            Ok(true)
        }
        Ok(false) if sign_path.is_file() => Ok(false),
        Ok(false) => Err("Path is not a valid repo".into()),
        Err(e) => Err(format!("failed to inspect {}: {e}", dir.display())),
    }
}

fn read_index(root: &str) -> Result<RepoTree, String> {
    let sign_path = Path::new(root).join(".ezrepo");
    let text = fs::read_to_string(&sign_path)
        .map_err(|e| format!("Failed when reading .ezrepo files: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("Failed when reading string: {e}"))
}

/// Atomic write: temp file + rename (Windows rename overwrites), so never a half-written file.
pub(crate) fn write_text_atomic(path: &Path, text: &str) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text).map_err(|e| format!("failed to write temporary file: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("failed to replace file: {e}"))
}

pub(crate) fn write_json_atomic<T: serde::Serialize>(
    path: &Path,
    value: &T,
    what: &str,
) -> Result<(), String> {
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| format!("Failed when serializing {what}: {e}"))?;
    write_text_atomic(path, &text)
}

fn write_index(root: &str, index: &RepoTree) -> Result<(), String> {
    write_json_atomic(&Path::new(root).join(".ezrepo"), index, ".ezrepo")
}

pub(crate) fn pdf_not_found(id: &str) -> String {
    format!("PDF id not found in repo: {id}")
}

/// Snapshot an index entry by id (read-only entry point for load_pdf / parse::load_bind).
pub(crate) fn find_pdf(root: &str, id: &str) -> Result<PDFStruct, String> {
    read_index(root)?
        .pdfs
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| pdf_not_found(id))
}

/// Disk naming (minted at import, collision-free): `<name>-<id>.pdf` / `<name>-<id>.json`.
fn pdf_file_name(name: &str, id: &str) -> String {
    format!("{name}-{id}.pdf")
}

fn bind_file_name(name: &str, id: &str) -> String {
    format!("{name}-{id}.json")
}

/// Lexical repo-relative check (`.` allowed, `..`/absolute rejected): stops hand-written .ezrepo values escaping the root.
fn check_relative(rel: &str) -> Result<(), String> {
    let ok = !rel.is_empty()
        && Path::new(rel)
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    if ok {
        Ok(())
    } else {
        Err(format!("rejected out-of-bounds repo-relative path: {rel}"))
    }
}

/// Resolve a bound-JSON path: lexical check + canonicalize inside the root (the latter catches symlink escapes).
fn resolve_bind_path(root: &str, rel: &str) -> Result<PathBuf, String> {
    check_relative(rel)?;
    let root_canon = fs::canonicalize(root).map_err(|e| format!("invalid repo root path: {e}"))?;
    let resolved = fs::canonicalize(root_canon.join(rel))
        .map_err(|e| format!("Failed when reading bound JSON: {e}"))?;
    if !resolved.starts_with(&root_canon) {
        return Err(format!("rejected out-of-bounds repo-relative path: {rel}"));
    }
    Ok(resolved)
}

#[tauri::command]
async fn gettree_from_config(app: tauri::AppHandle, root: &str) -> Result<RepoTree, String> {
    // Grant the asset protocol for this repo root only (static scope is empty → single least-privilege entry point).
    let index = read_index(root)?;
    app.asset_protocol_scope()
        .allow_directory(root, true)
        .map_err(|e| format!("Failed when allowing asset scope: {e}"))?;
    Ok(index)
}

pub(crate) fn read_bind_doc(abs: &Path) -> Result<BindDoc, String> {
    let text = fs::read_to_string(abs).map_err(|e| format!("Failed when reading bound JSON: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("Failed when parsing bound JSON: {e}"))
}

// Open a PDF by its stable id.
#[tauri::command]
async fn load_pdf(_root: &str, _id: &str) -> Result<PDF, String> {
    let dir = Path::new(_root);
    let entry = find_pdf(_root, _id)?;

    let name = entry.name.clone();
    let pdf_name = pdf_file_name(&name, &entry.id);
    check_relative(&pdf_name)?; // a hand-written .ezrepo can smuggle ../ via name/id
    let pdf_path = dir.join(&pdf_name).to_string_lossy().to_string();
    let (status, pages) = match &entry.bind {
        Some(rel) => {
            let json_abs = resolve_bind_path(_root, rel)?;
            let mut doc = read_bind_doc(&json_abs)?;
            // Backfill table grids for older JSONs and persist (same per-book lock as translation batches, so no clobbering).
            let lock = parse::file_lock(_root, &_id);
            let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
            if parse::backfill_table_grids(&mut doc) {
                if let Err(e) = parse::write_bind_atomic(&json_abs, &doc) {
                    eprintln!("Failed to persist table grids for {_id}: {e}");
                }
            }
            (doc.status, doc.pages)
        }
        None => (PDFStatus::Pending, Vec::new()),
    };

    Ok(PDF {
        id: entry.id,
        name,
        pdf_path,
        bind: entry.bind,
        status,
        pages,
    })
}

/// 12 hex-char content id (same content = same id, blocks duplicates); DefaultHasher may vary across versions but ids need only repo-local uniqueness.
fn content_id(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())[..12].to_string()
}

/// Real page count via lopdf (incl. xref/object streams); encrypted/corrupt → None (empty skeleton fallback).
fn pdf_page_count(bytes: &[u8]) -> Option<u32> {
    let doc = lopdf::Document::load_mem(bytes).ok()?;
    u32::try_from(doc.get_pages().len()).ok()
}

/// Import one file; the bound JSON pre-fills pages from the real count (empty on unreadable).
fn import_one(
    dir: &Path,
    belong: Option<&str>,
    src_path: &str,
    index: &RepoTree,
) -> Result<(PDFStruct, Option<String>), String> {
    let src = Path::new(src_path);
    if !src.is_file() {
        return Err("file not found".into());
    }
    let is_pdf = src
        .extension()
        .map(|e| e.to_string_lossy().eq_ignore_ascii_case("pdf"))
        .unwrap_or(false);
    if !is_pdf {
        return Err("only PDF files are supported".into());
    }
    let name = src
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.is_empty() || name.contains(['/', '\\', ':']) {
        return Err("invalid file name".into());
    }

    let bytes = fs::read(src).map_err(|e| format!("failed to read: {e}"))?;
    let id = content_id(&bytes);
    if index.pdfs.iter().any(|p| p.id == id) {
        return Err("content duplicates an already-imported PDF".into());
    }

    // name-id naming: same-named files from different dirs don't collide.
    let pdf_name = pdf_file_name(&name, &id);
    let json_name = bind_file_name(&name, &id);
    fs::write(dir.join(&pdf_name), &bytes).map_err(|e| format!("failed to store into repo: {e}"))?;
    let page_count = pdf_page_count(&bytes);
    let pages = match page_count {
        Some(n) => parse::build_skeleton(n)?,
        None => Vec::new(), // parse failed (e.g. encrypted): empty skeleton, filled when opened
    };
    let doc = BindDoc {
        status: PDFStatus::Pending,
        pages,
    };
    let json_text = serde_json::to_string_pretty(&doc)
        .map_err(|e| format!("Failed when serializing bound JSON: {e}"))?;
    fs::write(dir.join(&json_name), json_text).map_err(|e| format!("failed to write bind JSON: {e}"))?;

    let warning =
        page_count.is_none().then(|| "failed to parse page count (possibly encrypted or non-standard PDF); pages prefill skipped".into());
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
                    // Same validation as move_pdf: an overly long label would wreck the index.
                    if let Err(reason) = check_folder_name(b) {
                        failed.push(ImportFailure { path: src.clone(), reason });
                        continue;
                    }
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

// ---- Repo entry CRUD: folders are logical labels only; PDFs stay flat in the root; folder delete cascades ----

fn check_folder_name(name: &str) -> Result<(), String> {
    let bad = name.trim().is_empty()
        || name.contains(['/', '\\', ':'])
        || name == "."
        || name == ".."
        || name.chars().count() > 64;
    if bad {
        Err("invalid folder name (must be non-empty, no path separators, max 64 characters)".into())
    } else {
        Ok(())
    }
}

fn ensure_folder(index: &mut RepoTree, name: &str) {
    if !index.folders.iter().any(|f| f == name) {
        index.folders.push(name.to_string());
    }
}

#[tauri::command]
fn create_folder(root: &str, name: String) -> Result<RepoTree, String> {
    let name = name.trim().to_string();
    check_folder_name(&name)?;
    let mut index = read_index(root)?;
    if index.folders.iter().any(|f| f == &name) {
        return Err(format!("folder with the same name already exists: {name}"));
    }
    index.folders.push(name);
    write_index(root, &index)?;
    Ok(index)
}

/// Best-effort file delete: the index is authoritative, so missing/out-of-bounds paths are non-fatal.
fn remove_pdf_files(root: &str, entry: &PDFStruct) {
    let pdf_name = pdf_file_name(&entry.name, &entry.id);
    if check_relative(&pdf_name).is_ok() {
        let _ = fs::remove_file(Path::new(root).join(&pdf_name));
    }
    if let Some(rel) = &entry.bind {
        if let Ok(abs) = resolve_bind_path(root, rel) {
            let _ = fs::remove_file(abs);
        }
    }
}

/// Delete a folder and cascade over its PDFs (the confirm dialog says so); racy batches abort on the deleted JSON, so data can't come back.
#[tauri::command]
fn delete_folder(root: &str, name: String) -> Result<RepoTree, String> {
    let mut index = read_index(root)?;
    let pos = index
        .folders
        .iter()
        .position(|f| f == &name)
        .ok_or_else(|| format!("folder does not exist: {name}"))?;
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

#[tauri::command]
fn delete_pdf(root: &str, id: &str) -> Result<RepoTree, String> {
    let mut index = read_index(root)?;
    let pos = index
        .pdfs
        .iter()
        .position(|p| p.id == id)
        .ok_or_else(|| pdf_not_found(id))?;
    let entry = index.pdfs.remove(pos);
    remove_pdf_files(root, &entry);
    write_index(root, &index)?;
    Ok(index)
}

/// Move a PDF: belong=Some moves it into a folder (auto-created), None moves it to the root.
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
        .ok_or_else(|| pdf_not_found(id))?;
    entry.belong = belong;
    write_index(root, &index)?;
    Ok(index)
}

// ---- Settings persistence ----

/// Windows prod = beside the exe (NSIS per-user, writable); Linux/macOS prod = ~/.ezpdf/config.json (install dir read-only); dev = repo root.
fn settings_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    #[cfg(dev)]
    {
        let _ = app;
        Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../config.json"))
    }
    #[cfg(not(dev))]
    {
        prod_settings_path(app)
    }
}

/// Production settings path (separate fn so dev builds compile-check it): beside the exe on Windows, else ~/.ezpdf/config.json.
#[cfg_attr(dev, allow(dead_code))]
fn prod_settings_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if cfg!(windows) {
        let exe = std::env::current_exe().map_err(|e| format!("failed to locate application path: {e}"))?;
        let dir = exe.parent().ok_or("failed to resolve application directory")?;
        return Ok(dir.join("config.json"));
    }
    let home = app
        .path()
        .home_dir()
        .map_err(|e| format!("failed to locate user home directory: {e}"))?;
    Ok(home.join(".ezpdf").join("config.json"))
}

/// Read settings; missing file → Ok(None) so the frontend keeps defaults and still starts.
#[tauri::command]
fn load_settings(app: tauri::AppHandle) -> Result<Option<String>, String> {
    let path = settings_path(&app)?;
    match fs::read_to_string(&path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("failed to read settings {}: {e}", path.display())),
    }
}

/// Settings size cap (real config.json is a few KB) to stop runaway frontend writes.
const MAX_SETTINGS_BYTES: usize = 8 * 1024 * 1024;

/// Write settings (JSON validated, temp-file atomic replace); the frontend sends the whole file.
#[tauri::command]
fn save_settings(app: tauri::AppHandle, json: String) -> Result<(), String> {
    if json.len() > MAX_SETTINGS_BYTES {
        return Err(format!("settings payload too large: {} bytes", json.len()));
    }
    serde_json::from_str::<serde_json::Value>(&json).map_err(|e| format!("invalid settings JSON: {e}"))?;
    let path = settings_path(&app)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("failed to create settings directory: {e}"))?;
    }
    write_text_atomic(&path, &json)
}

// ---- OCR service commands (details in pyenv.rs / pyserver.rs) ----

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
) -> Result<bool, String> {
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
        return Ok(()); // ignore duplicate starts while Starting/Connected
    }
    svc.reset_for_start();
    let app2 = app.clone();
    let paths2 = paths.inner().clone();
    let svc2 = svc.inner().clone();
    tauri::async_runtime::spawn(pyserver::supervise(app2, paths2, svc2));
    Ok(())
}

#[tauri::command]
async fn ocr_stop(
    app: tauri::AppHandle,
    svc: tauri::State<'_, pyserver::PyService>,
) -> Result<(), String> {
    if svc.has_child() {
        svc.stop(); // closing stdin makes Python exit; status is reported via ocr://status
    } else {
        svc.disconnect(&app); // remote mode: no child to stop, just clear the endpoint
    }
    Ok(())
}

/// Remote-mode probe ("Test" button): no state change; returns the server's advertised batch size.
#[tauri::command]
async fn ocr_health(url: String, token: String) -> Result<pyserver::ParseServiceHealth, String> {
    pyserver::probe_health(&url, &token).await
}

/// Remote-mode connect: register as the OCR target only after a successful probe.
#[tauri::command]
async fn ocr_start_remote(
    app: tauri::AppHandle,
    svc: tauri::State<'_, pyserver::PyService>,
    url: String,
    token: String,
) -> Result<pyserver::ParseServiceHealth, String> {
    let base = pyserver::normalize_base(&url)?;
    let health = pyserver::probe_health(&base, &token).await?;
    svc.set_remote(&app, base.clone(), token);
    let _ = app.emit(
        "ocr://log",
        format!(
            "[ezpdf] online parse service connected: {base} (pid {}, {} ms, max batch {} pages)",
            health.pid.map(|p| p.to_string()).unwrap_or_else(|| "?".into()),
            health.elapsed_ms,
            health.max_batch_pages
        ),
    );
    Ok(health)
}

// ---- OCR parse loop: batch OCR + skeleton prefill + per-batch LLM translation; details in parse.rs / translate.rs ----

/// One OCR batch (≤4 pages, same book): images → OCR → one atomic write; translate_pdf is fired concurrently.
#[tauri::command]
async fn parse_pdf(
    root: &str,
    id: &str,
    pages: Vec<parse::ParsePageInput>,
    svc: tauri::State<'_, pyserver::PyService>,
) -> Result<parse::ParseOutcome, String> {
    let (base, token) = svc
        .ocr_target()
        .ok_or_else(|| "OCR service not connected".to_string())?;
    parse::parse_batch(root, id, pages, &base, &token).await
}

/// Retry translation-only (no OCR) for finished && !translated pages.
#[tauri::command]
async fn translate_pdf(
    root: &str,
    id: &str,
    pages: Vec<u32>,
    llm: translate::LlmConfig,
) -> Result<parse::ParseOutcome, String> {
    parse::translate_batch(root, id, pages, &llm).await
}

/// Prefill on open: backfill pages for bound JSONs left empty by a failed lopdf parse, using pdfjs's real count.
#[tauri::command]
async fn prefill_pages(root: &str, id: &str, total: u32) -> Result<PDFStatus, String> {
    parse::prefill_pages(root, id, total).await
}

/// Clear a book's parse state: rebuild an empty skeleton, drop OCR blocks and translations; the PDF is untouched.
#[tauri::command]
fn reset_pdf_state(root: &str, id: &str, total: u32) -> Result<PDFStatus, String> {
    parse::reset_pdf_state(root, id, total)
}

/// LLM connectivity check + thinking-off strategy probe (the "Verify" button).
#[tauri::command]
async fn verify_llm(llm: translate::LlmConfig) -> Result<translate::LlmVerifyReport, String> {
    translate::verify_llm(&llm).await
}

/// Fetch the model list for autocomplete; `api` is the protocol, deciding path and auth headers.
#[tauri::command]
async fn fetch_llm_models(base_url: String, api_key: String, api: String) -> Result<Vec<String>, String> {
    translate::fetch_models(&base_url, &api_key, &api).await
}

/// Check for updates (latest GitHub Release vs app version); the frontend decides silent vs toast.
#[tauri::command]
async fn check_update(app: tauri::AppHandle) -> Result<update::UpdateInfo, String> {
    let current = app.package_info().version.to_string();
    update::check_update(&current).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let paths = pyenv::PyPaths::resolve(app.handle())?;
            println!("[ezpdf] PyPaths = {paths:?}");
            app.manage(paths);
            app.manage(pyserver::PyService::new());
            translate::init_log(app.handle()); // translation logs → llm://log
            translate::init_prompt_dir(app.handle()); // prompt dir: prod = install resources
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
            ocr_health,
            ocr_start_remote,
            parse_pdf,
            translate_pdf,
            prefill_pages,
            reset_pdf_state,
            verify_llm,
            fetch_llm_models,
            check_update,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // On exit close stdin so Python's stdin-EOF watchdog exits (no orphans).
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
        // Drive prefixes/backslashes escape only on Windows (on Unix `\` is ordinary), so these assertions are Windows-only.
        #[cfg(windows)]
        {
            assert!(check_relative(r"C:\evil\foo.json").is_err());
            assert!(check_relative(r"a-..\..\evil.pdf").is_err());
        }
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
        assert!(create_folder(&root_s, " 理论 ".into()).is_err()); // duplicate after trim
        let index = create_folder(&root_s, "实验".into()).unwrap();
        assert_eq!(index.folders, vec!["理论".to_string(), "实验".to_string()]);

        // Non-empty folder cascade: index entries + repo PDF/bound JSON are deleted too
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
        assert!(delete_folder(&root_s, "理论".into()).is_err()); // already deleted

        // Move a PDF: root → folder (auto-created, kept) → root
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
