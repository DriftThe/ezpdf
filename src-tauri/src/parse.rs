//! Batch OCR: offscreen page images → pyserver /ocr/pages → px→pt → one atomic bound-JSON write
//! per batch; translation is separate (translate_pdf, fired concurrently after each batch).
//!
//! parse_pdf handles ≤4 pages of one book: one read → one inference → one tmp+rename write, so a
//! crash loses at most one batch. The per-book lock covers only read-modify-write; network calls stay outside it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::translate::{self, LlmConfig};
use crate::table::TABLE;
use crate::{resolve_bind_path, BindDoc, Block, PageInfo, PDFStatus};

/// Per-book file lock (key = root/id) serializing concurrent OCR/translation writes.
static FILE_LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();

pub(crate) fn file_lock(root: &str, id: &str) -> Arc<Mutex<()>> {
    let map = FILE_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let key = format!("{root}/{id}");
    let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .entry(key)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// One parse_pdf input page (frontend offscreen render); index is 1-based, matching the bound JSON.
#[derive(Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ParsePageInput {
    pub index: u32,
    pub image_b64: String,
    pub scale: f64,
}

#[derive(Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ParseOutcome {
    pub book_status: PDFStatus,
    pub updated_pages: Vec<PageInfo>,
}

// ---- pyserver /ocr/pages response (only the needed fields) ----

#[derive(Deserialize)]
struct OcrBatchResponse {
    pages: Vec<OcrPageResponse>,
}

#[derive(Deserialize)]
struct OcrPageResponse {
    blocks: Vec<OcrBlockResponse>,
}

#[derive(Deserialize)]
struct OcrBlockResponse {
    label: String,
    bbox_px: [f64; 4],
    markdown: String,
}

/// bbox pixels → PDF points (pt = px/scale), rounded to 2 decimals.
fn px_to_pt(v: f64, scale: f64) -> f64 {
    ((v / scale) * 100.0).round() / 100.0
}

/// OCR block → Block: label kept as-is; image gets empty content (figure contract); translation None (bypass stage).
fn map_blocks(blocks: &[OcrBlockResponse], scale: f64) -> Vec<Block> {
    blocks
        .iter()
        .map(|b| Block {
            kind: b.label.clone(),
            content: if b.label == "image" {
                String::new()
            } else {
                b.markdown.clone()
            },
            loc: b.bbox_px.map(|v| px_to_pt(v, scale)),
            translation: None,
            grid: None,
        })
        .collect()
}

/// Patch (index, blocks) into the BindDoc: finished=true, translated=false (re-OCR needs re-translation); unknown indexes skipped.
fn patch_pages(doc: &mut BindDoc, updates: Vec<(u32, Vec<Block>)>) -> Vec<u32> {
    let mut patched = Vec::new();
    for (index, blocks) in updates {
        if let Some(page) = doc.pages.iter_mut().find(|p| p.index == index) {
            page.blocks = blocks;
            page.finished = true;
            page.translated = false;
            patched.push(index);
        }
    }
    patched
}

/// Status transition (Rust is sole writer): all pages finished (≥1) → Finished; Pending → Processing; else stay put.
fn finalize_status(doc: &mut BindDoc) {
    if !doc.pages.is_empty() && doc.pages.iter().all(|p| p.finished) {
        doc.status = PDFStatus::Finished;
    } else if matches!(doc.status, PDFStatus::Pending) {
        doc.status = PDFStatus::Processing;
    }
}

/// Backfill `grid` on pre-table JSONs so the frontend can render/re-translate them; parsed once on load (no LLM).
pub(crate) fn backfill_table_grids(doc: &mut BindDoc) -> bool {
    let mut changed = false;
    for page in doc.pages.iter_mut() {
        for block in page.blocks.iter_mut() {
            if block.kind != TABLE || block.grid.is_some() {
                continue;
            }
            if let Some(grid) = crate::table::parse_markup(&block.content) {
                block.grid = Some(grid);
                changed = true;
            }
        }
    }
    changed
}

/// Atomic bound-JSON write: tmp + rename (Windows rename = MOVEFILE_REPLACE_EXISTING).
pub(crate) fn write_bind_atomic(json_path: &Path, doc: &BindDoc) -> Result<(), String> {
    crate::write_json_atomic(json_path, doc, "bound JSON")
}

fn load_bind(root: &str, id: &str) -> Result<(PathBuf, BindDoc), String> {
    let entry = crate::find_pdf(root, id)?;
    let bind = entry
        .bind
        .as_ref()
        .ok_or_else(|| format!("PDF has no bound JSON: {id}"))?;
    let json_path = resolve_bind_path(root, bind)?;
    let doc = crate::read_bind_doc(&json_path)?;
    Ok((json_path, doc))
}

fn pages_by_index(doc: &BindDoc, indices: &[u32]) -> Vec<PageInfo> {
    indices
        .iter()
        .filter_map(|i| doc.pages.iter().find(|p| p.index == *i).cloned())
        .collect()
}

fn outcome(doc: &BindDoc, updated: Vec<PageInfo>) -> ParseOutcome {
    ParseOutcome {
        book_status: doc.status,
        updated_pages: updated,
    }
}

/// POST /ocr/pages, patch pages, transition status, one atomic write; translation follows via translate_pdf.
pub async fn parse_batch(
    root: &str,
    id: &str,
    pages: Vec<ParsePageInput>,
    base: &str,
    token: &str,
) -> Result<ParseOutcome, String> {
    if pages.is_empty() {
        return Err("OCR batch is empty".into());
    }
    if pages.len() > MAX_BATCH_PAGES as usize {
        return Err(format!(
            "batch page count exceeds limit: {} > {MAX_BATCH_PAGES}",
            pages.len()
        ));
    }

    // Fail fast: no need to run the model if the book/bound JSON is missing.
    {
        let lock = file_lock(root, id);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let _ = load_bind(root, id)?;
    }

    // No total timeout: with lazy engine loading the first request can take minutes.
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "pages": pages
            .iter()
            .map(|p| serde_json::json!({ "image_b64": p.image_b64 }))
            .collect::<Vec<_>>()
    });
    let resp = client
        .post(format!("{base}/ocr/pages"))
        .header("x-ezpdf-token", token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("OCR service request failed: {e}"))?;
    let http = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("failed to read OCR service response: {e}"))?;
    if !http.is_success() {
        return Err(format!("OCR service returned {http}: {}", truncate(&text, 300)));
    }
    let ocr: OcrBatchResponse = serde_json::from_str(&text)
        .map_err(|e| format!("failed to parse OCR service response: {e}"))?;
    if ocr.pages.len() != pages.len() {
        return Err(format!(
            "OCR page count mismatch: requested {} got {}",
            pages.len(),
            ocr.pages.len()
        ));
    }

    // Persist under lock: re-read (merging concurrent translation writes) → patch → atomic write.
    let result = {
        let lock = file_lock(root, id);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let (json_path, mut doc) = load_bind(root, id)?;

        let updates: Vec<(u32, Vec<Block>)> = pages
            .iter()
            .zip(&ocr.pages)
            .map(|(p, r)| (p.index, map_blocks(&r.blocks, p.scale)))
            .collect();
        let total_blocks: usize = updates.iter().map(|(_, b)| b.len()).sum();
        let touched = patch_pages(&mut doc, updates);
        translate::log(format!(
            "OCR done p{:?}: {total_blocks} blocks",
            pages.iter().map(|p| p.index).collect::<Vec<_>>()
        ));

        let updated = pages_by_index(&doc, &touched);
        finalize_status(&mut doc);
        write_bind_atomic(&json_path, &doc)?;
        outcome(&doc, updated)
    };

    Ok(result)
}

/// Translation phase grouping (stride-2 concurrency): positions 0,2,4… in one phase, 1,3,5… in the other.
fn phase_pages(indices: &[u32], phase: usize) -> Vec<u32> {
    indices
        .iter()
        .enumerate()
        .filter(|(i, _)| i % 2 == phase)
        .map(|(_, v)| *v)
        .collect()
}

/// Translate-only retry for finished && !translated pages. Stride-2 phases keep neighbours non-concurrent (context can't collide); phase 2 sees phase 1's persisted bindings.
pub async fn translate_batch(
    root: &str,
    id: &str,
    indices: Vec<u32>,
    llm: &LlmConfig,
) -> Result<ParseOutcome, String> {
    // Each page is a paid LLM request: cap the count and dedupe so bad frontend args can't fan out.
    let mut indices = indices;
    indices.sort_unstable();
    indices.dedup();
    if indices.is_empty() {
        return Err("translate batch is empty".into());
    }
    if indices.len() > MAX_BATCH_PAGES as usize {
        return Err(format!(
            "batch page count exceeds limit: {} > {MAX_BATCH_PAGES}",
            indices.len()
        ));
    }
    // Translation disabled: no network, source text is stored as the translation and marked done.
    if !llm.translate_enabled {
        return bypass_batch(root, id, &indices, llm);
    }
    if !llm.usable() {
        return Err("LLM not configured (baseUrl/apiKey/model empty)".into());
    }
    let prompt = translate::load_system_prompt(llm)?;
    let client = translate::llm_client()?;
    translate::log(format!(
        "retry batch p{indices:?} (model={} smart={})",
        llm.model, llm.smart_context
    ));

    let mut touched: Vec<u32> = Vec::new();
    for phase in 0..2 {
        let phase_pages = phase_pages(&indices, phase);
        if phase_pages.is_empty() {
            continue;
        }

        // Snapshot under lock.
        let tasks: Vec<translate::PageTask> = {
            let lock = file_lock(root, id);
            let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
            let (_, doc) = load_bind(root, id)?;
            phase_pages
                .iter()
                .filter_map(|&i| translate::build_task(llm, &doc, i))
                .collect()
        };
        if tasks.is_empty() {
            continue;
        }

        // Concurrent translation (outside the lock; pages in a phase are non-adjacent).
        let mut handles = Vec::with_capacity(tasks.len());
        for task in tasks {
            let (cfg, prompt, client) = (llm.clone(), prompt.clone(), client.clone());
            handles.push(tauri::async_runtime::spawn(async move {
                translate::translate_task(&cfg, &prompt, &client, task).await
            }));
        }
        let mut done_list = Vec::new();
        for handle in handles {
            match handle.await {
                Ok(Ok(done)) => done_list.push(done),
                Ok(Err(e)) => translate::log(e),
                Err(e) => translate::log(format!("translation task exited abnormally: {e}")),
            }
        }
        if done_list.is_empty() {
            continue;
        }

        // Persist under lock.
        let lock = file_lock(root, id);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let (json_path, mut doc) = load_bind(root, id)?;
        for done in done_list {
            translate::apply_done(&mut doc, done, &mut touched);
        }
        if !touched.is_empty() {
            write_bind_atomic(&json_path, &doc)?;
        }
    }

    touched.sort_unstable();
    touched.dedup();
    let (_, doc) = {
        let lock = file_lock(root, id);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        load_bind(root, id)?
    };
    Ok(outcome(&doc, pages_by_index(&doc, &touched)))
}

/// Disabled-translation path: no network, source text copied into translation and the page marked done in one write.
fn bypass_batch(
    root: &str,
    id: &str,
    indices: &[u32],
    llm: &LlmConfig,
) -> Result<ParseOutcome, String> {
    let touched = {
        let lock = file_lock(root, id);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let (json_path, mut doc) = load_bind(root, id)?;
        let mut touched: Vec<u32> = Vec::new();
        for &i in indices {
            if translate::apply_bypass(&mut doc, i, llm) {
                touched.push(i);
            }
        }
        touched.sort_unstable();
        touched.dedup();
        if !touched.is_empty() {
            write_bind_atomic(&json_path, &doc)?;
            translate::log(format!("translation disabled: copied OCR text for p{touched:?}"));
        }
        touched
    };
    let (_, doc) = {
        let lock = file_lock(root, id);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        load_bind(root, id)?
    };
    Ok(outcome(&doc, pages_by_index(&doc, &touched)))
}

/// When pages is empty (failed lopdf parse) rebuild a 1..=N skeleton from the real count; never touches existing pages.
pub async fn prefill_pages(root: &str, id: &str, total: u32) -> Result<PDFStatus, String> {
    let (json_path, mut doc) = load_bind(root, id)?;
    if !doc.pages.is_empty() || total == 0 {
        return Ok(doc.status);
    }
    doc.pages = build_skeleton(total)?;
    write_bind_atomic(&json_path, &doc)?;
    Ok(doc.status)
}

/// Drop all blocks/translations and rebuild a 1..=N skeleton (PDF untouched); total=0 (book not open) reuses the JSON count.
pub fn reset_pdf_state(root: &str, id: &str, total: u32) -> Result<PDFStatus, String> {
    let lock = file_lock(root, id);
    let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
    let (json_path, mut doc) = load_bind(root, id)?;
    let count = if total > 0 { total } else { doc.pages.len() as u32 };
    if count == 0 {
        return Err("page count unknown: open the PDF once before clearing".into());
    }
    doc.pages = build_skeleton(count)?;
    doc.status = PDFStatus::Pending;
    write_bind_atomic(&json_path, &doc)?;
    Ok(doc.status)
}

/// Hard batch limit (advertised values clamp to it); guards malformed counts from huge Vecs (panic=abort, so OOM kills).
pub(crate) const MAX_BATCH_PAGES: u32 = 32;

/// Page-count cap for import prefill (malformed-PDF guard).
const MAX_PAGES: u32 = 20_000;

/// 1..=N empty page skeleton (shared by import prefill and open-time backfill).
pub(crate) fn build_skeleton(total: u32) -> Result<Vec<PageInfo>, String> {
    if total > MAX_PAGES {
        return Err(format!("page count out of range: {total} > {MAX_PAGES}"));
    }
    Ok((1..=total)
        .map(|i| PageInfo {
            index: i,
            finished: false,
            translated: false,
            blocks: Vec::new(),
        })
        .collect())
}

/// Truncate a single line to ≤cap chars (log/error summaries; char-boundary safe).
pub(crate) fn truncate(s: &str, cap: usize) -> &str {
    match s.char_indices().nth(cap) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Backfill: only parsable tables get a grid (unparsable stay None, not re-translation targets).
    #[test]
    fn backfill_fills_only_parsable_tables() {
        let blk = |content: &str, grid| Block {
            kind: "table".into(),
            content: content.into(),
            loc: [0.0; 4],
            translation: None,
            grid,
        };
        let mut d = BindDoc {
            status: PDFStatus::Finished,
            pages: vec![PageInfo {
                index: 1,
                finished: true,
                translated: true,
                blocks: vec![blk("<fcel>a<fcel>b<nl>", None), blk("没有表格标记", None)],
            }],
        };
        assert!(backfill_table_grids(&mut d), "first pass writes the parsable one");
        assert_eq!(d.pages[0].blocks[0].grid.as_ref().map(|g| g.cols), Some(2));
        assert!(d.pages[0].blocks[1].grid.is_none());
        assert!(!backfill_table_grids(&mut d), "second pass is a no-op");
    }

    #[test]
    fn reset_pdf_state_rebuilds_skeleton() {
        let root = std::env::temp_dir().join(format!("ezpdf-reset-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let bind = "Book-ab12.json";
        std::fs::write(
            root.join(".ezrepo"),
            format!(r#"{{"folders":[],"pdfs":[{{"id":"ab12","name":"Book","bind":"{bind}","belong":null}}]}}"#),
        )
        .unwrap();
        let full = std::fs::canonicalize(&root).unwrap();
        let path = full.join(bind);
        let doc = BindDoc {
            status: PDFStatus::Finished,
            pages: vec![page_for_reset(1), page_for_reset(2)],
        };
        write_bind_atomic(&path, &doc).unwrap();

        let status = reset_pdf_state(full.to_str().unwrap(), "ab12", 3).unwrap();
        assert!(matches!(status, PDFStatus::Pending));
        let after = crate::read_bind_doc(&path).unwrap();
        assert_eq!(after.pages.len(), 3, "page count comes from the caller");
        assert!(after.pages.iter().all(|p| !p.finished && !p.translated && p.blocks.is_empty()));

        // total = 0 → reuse the page count already in the JSON
        let status = reset_pdf_state(full.to_str().unwrap(), "ab12", 0).unwrap();
        assert!(matches!(status, PDFStatus::Pending));
        assert_eq!(crate::read_bind_doc(&path).unwrap().pages.len(), 3);

        let _ = std::fs::remove_dir_all(&root);
    }

    fn page_for_reset(index: u32) -> PageInfo {
        PageInfo {
            index,
            finished: true,
            translated: true,
            blocks: vec![Block {
                kind: "text".into(),
                content: "hello".into(),
                loc: [0.0; 4],
                translation: Some("你好".into()),
                grid: None,
            }],
        }
    }

    fn ocr_block(label: &str, bbox_px: [f64; 4], markdown: &str) -> OcrBlockResponse {
        OcrBlockResponse {
            label: label.into(),
            bbox_px,
            markdown: markdown.into(),
        }
    }

    fn page(index: u32, finished: bool) -> PageInfo {
        PageInfo {
            index,
            finished,
            translated: false,
            blocks: Vec::new(),
        }
    }

    #[test]
    fn px_to_pt_converts_and_rounds() {
        assert_eq!(px_to_pt(100.0, 2.0), 50.0);
        assert_eq!(px_to_pt(3.0, 2.0), 1.5);
        assert_eq!(px_to_pt(1.0, 3.0), 0.33);
        assert_eq!(px_to_pt(595.5, 2.0), 297.75);
    }

    #[test]
    fn map_blocks_maps_fields_and_image_empty() {
        let blocks = vec![
            ocr_block("text", [0.0, 0.0, 200.0, 40.0], "hello"),
            ocr_block("image", [10.0, 10.0, 100.0, 100.0], "ignored"),
            ocr_block("table", [0.0, 0.0, 100.0, 50.0], "<fcel>a<nl>"),
        ];
        let out = map_blocks(&blocks, 2.0);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].kind, "text");
        assert_eq!(out[0].content, "hello");
        assert_eq!(out[0].loc, [0.0, 0.0, 100.0, 20.0]);
        // image → empty content (figure contract), translation None (bypass stage)
        assert_eq!(out[1].kind, "image");
        assert!(out[1].content.is_empty());
        assert!(out[1].translation.is_none());
        assert_eq!(out[2].kind, "table");
        assert_eq!(out[2].content, "<fcel>a<nl>");
    }

    #[test]
    fn patch_pages_updates_and_skips_unknown() {
        let mut doc = BindDoc {
            status: PDFStatus::Pending,
            pages: vec![page(1, false), page(2, false)],
        };
        let blk = |md: &str| Block {
            kind: "text".into(),
            content: md.into(),
            loc: [0.0; 4],
            translation: None,
            grid: None,
        };
        let patched = patch_pages(
            &mut doc,
            vec![(2, vec![blk("b")]), (99, vec![])],
        );
        assert_eq!(patched, vec![2]);
        assert!(!doc.pages[0].finished);
        assert!(doc.pages[1].finished);
        assert!(!doc.pages[1].translated); // freshly OCR'd page needs re-translation
        assert_eq!(doc.pages[1].blocks.len(), 1);
    }

    #[test]
    fn finalize_status_transitions() {
        let mut doc = BindDoc {
            status: PDFStatus::Pending,
            pages: vec![page(1, false), page(2, true)],
        };
        finalize_status(&mut doc);
        assert!(matches!(doc.status, PDFStatus::Processing));

        doc.pages[0].finished = true;
        finalize_status(&mut doc);
        assert!(matches!(doc.status, PDFStatus::Finished));

        doc.pages.clear();
        doc.status = PDFStatus::Pending;
        finalize_status(&mut doc);
        assert!(matches!(doc.status, PDFStatus::Processing));

        // Processing stays put
        doc.status = PDFStatus::Processing;
        doc.pages = vec![page(1, false)];
        finalize_status(&mut doc);
        assert!(matches!(doc.status, PDFStatus::Processing));
    }

    #[test]
    fn build_skeleton_fills_1_to_n() {
        let pages = build_skeleton(3).unwrap();
        assert_eq!(pages.iter().map(|p| p.index).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert!(pages.iter().all(|p| !p.finished && p.blocks.is_empty()));
        // cap: malformed page counts are rejected outright
        assert!(build_skeleton(MAX_PAGES).is_ok());
        assert!(build_skeleton(MAX_PAGES + 1).is_err());
    }

    #[test]
    fn phase_pages_stride_two() {
        assert_eq!(phase_pages(&[1, 2, 3, 4], 0), vec![1, 3]);
        assert_eq!(phase_pages(&[1, 2, 3, 4], 1), vec![2, 4]);
        assert_eq!(phase_pages(&[5, 6, 7, 8, 9], 0), vec![5, 7, 9]);
        assert_eq!(phase_pages(&[5, 6, 7, 8, 9], 1), vec![6, 8]);
        assert!(phase_pages(&[], 0).is_empty());
        assert!(phase_pages(&[7], 1).is_empty());
    }
}
