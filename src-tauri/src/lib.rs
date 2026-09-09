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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EZrepoConfig {
    pub folder: Option<Vec<String>>,
    pub pdfs: Option<Vec<PDFStruct>>,
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
