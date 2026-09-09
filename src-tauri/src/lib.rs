// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;
use ts_rs::TS;

#[derive(Deserialize, Serialize, TS)]
#[ts(export)]
pub struct RepoTree {
    pub folders: Vec<String>,
    pub pdfs: Vec<PDFStruct>,
}

#[derive(Serialize, TS, Deserialize)]
#[ts(export)]
pub struct PDFStruct {
    pub name:String,
    pub bind: Option<String>,
    pub belong: Option<String>,
}

// ---- ezpdf 书实体域模型（load_book 传输载荷；ts-rs 导出到 bindings/，前端 domain.ts re-export）----

/// 页解析状态机：pending → ocr_queued → ocr_done → translating → done / failed
#[derive(Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum PageStatus {
    Pending,
    OcrQueued,
    OcrDone,
    Translating,
    Done,
    Failed,
}

/// [x, y, w, h]，单位 PDF 点（1pt = 1/72in），左上原点，scale=1 视口（与缩放无关）
#[derive(Deserialize, Serialize, TS)]
#[ts(export)]
pub struct BboxPt(pub [f64; 4]);

/// 版面块：OCR 检出的一个区域及其原文/译文。
/// label 为 PP-DocLayoutV3 标签（text/title/list/figure/figure_caption/table/formula/header/footer），
/// 保留 string 通道以兼容后续新增标签，故不用 enum。
#[derive(Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    pub id: String, // 如 "p3-b12"
    pub label: String,
    pub bbox_pt: BboxPt,
    pub score: f32,
    /// markdown 原文：正文纯文本 / 公式 $$..$$ / 表格 markdown；figure 块为 None
    pub source: Option<String>,
    /// markdown 译文；figure/formula 块为 None（formula 原样渲染），未译为 None
    pub translation: Option<String>,
}

#[derive(Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    pub index: u32, // 0-based
    pub status: PageStatus,
    pub width_pt: f64,
    pub height_pt: f64,
    pub blocks: Vec<Block>,
}

#[derive(Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct BookMeta {
    pub name: String,
    pub page_count: u32,
    pub created_at: String, // ISO 8601
    pub updated_at: String,
    pub target_lang: String,
    pub ocr_engine: String,
    pub llm_model: String,
}

/// 一本书的完整实体（= 书目录里 <书名>.json 的形状）。
/// id 前端会归一化为索引键 belong/name；bind = 结构 JSON 的仓库相对路径，None = 未解析。
#[derive(Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Book {
    pub id: String,
    pub name: String,
    pub pdf_path: String,
    pub json_path: String,
    pub bind: Option<String>,
    pub meta: BookMeta,
    pub pages: Vec<PageInfo>,
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
            fs::write(&_sign_path,r#"{"folders":[],"pdfs":[]}"#).map_err(|e| format!("Failed when creating .ezrepo file:{e}"))?;
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

//Get repoTree from config
#[tauri::command]
async fn gettree_from_config(root: &str) -> Result<RepoTree, String> {
    let dir = Path::new(root);
    let sign_path = Path::new(dir).join(".ezrepo");
    let text = fs::read_to_string(&sign_path)
        .map_err(|e| format!("Failed when reading .ezrepo files: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("Failed when reading string: {e}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            check_and_build_repo,
            gettree_from_config
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
