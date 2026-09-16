//! OCR 批量解析（阶段4）：前端离屏渲染页图 → 批量调 pyserver /ocr/pages →
//! px→pt 映射 → 整批一次原子写回绑定 JSON。翻译走独立的 translate_pdf（前端在
//! OCR 批次返回后立即排队并发，见 PLAN-LLM.md §4）。
//!
//! 批次语义（用户拍板）：parse_pdf 每批 ≤4 页、同一本书；本模块每批恰好
//! 一次读 JSON → 一次批量推理 → 一次 tmp+rename 原子写——崩溃最多丢一批
//! （≤4 页），JSON 恒为完整一致快照，永不半写。
//! OCR 与翻译可能并发落盘：锁只包住"读盘 → 改 → 原子写"小段（网络/模型调用在锁外）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::translate::{self, LlmConfig};
use crate::table::TABLE;
use crate::{resolve_bind_path, BindDoc, Block, PageInfo, PDFStatus};

/// 书级文件锁（键 = root/id）：OCR 批次与翻译批次并发落盘时序列化读改写
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

/// parse_pdf 批次输入页（前端离屏渲染产物；index 1-based，与绑定 JSON 对齐）
#[derive(Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ParsePageInput {
    pub index: u32,
    pub image_b64: String,
    pub scale: f64,
}

/// parse_pdf 返回：进度摘要 + 本批实际更新的页
#[derive(Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ParseOutcome {
    pub book_status: PDFStatus,
    pub updated_pages: Vec<PageInfo>,
}

// ---- pyserver /ocr/pages 响应（只取需要的字段）----

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

/// bbox 像素 → PDF pt（pt = px/scale），保留 2 位小数
fn px_to_pt(v: f64, scale: f64) -> f64 {
    ((v / scale) * 100.0).round() / 100.0
}

/// OCR 响应块 → 绑定 JSON Block：label 原样（string 通道，PP-DocLayoutV3
/// 实测输出 text/paragraph_title/doc_title/table/formula/image/chart/
/// abstract/reference/reference_content/footer/header/footnote/seal/number
/// 等 20 类）；image 块 content 置空串（figure 契约）；translation=None（bypass）
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

/// 把一批 (index, blocks) patch 进 BindDoc：整批 finished=true、translated=false
/// （重新 OCR 的页需重翻）；索引不在骨架内的页静默跳过；返回实际更新页的 index
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

/// 书级状态迁移（Rust 单写者职责）：
/// - 全页 finished（且至少一页）→ Finished
/// - 尚未开始（Pending）→ Processing（批间残留状态；崩溃后重启据此续跑）
/// - 已是 Processing/Finished → 保持
fn finalize_status(doc: &mut BindDoc) {
    if !doc.pages.is_empty() && doc.pages.iter().all(|p| p.finished) {
        doc.status = PDFStatus::Finished;
    } else if matches!(doc.status, PDFStatus::Pending) {
        doc.status = PDFStatus::Processing;
    }
}

/// 补齐表块网格（用户 2026-09-16）：表格支持之前解析的 JSON 没有 `grid`，
/// 前端既无法渲染、也无法判断"哪些表格还值得翻"。load_pdf 时顺带解析落盘一次
/// （纯派生数据，不涉及 LLM），之后前端就能只挑**可解析且未翻**的表格补翻。
/// 返回是否有变更（无变更不写盘）。
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

/// 原子写绑定 JSON：tmp + rename（Windows fs::rename = MOVEFILE_REPLACE_EXISTING，
/// 覆盖已存在目标）
pub(crate) fn write_bind_atomic(json_path: &Path, doc: &BindDoc) -> Result<(), String> {
    crate::write_json_atomic(json_path, doc, "bound JSON")
}

/// 读索引定位绑定 JSON 并解析为 BindDoc（parse_batch / prefill_pages 共用入口）
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

/// 文档中指定 index 的页快照（updatedPages 回传用）
fn pages_by_index(doc: &BindDoc, indices: &[u32]) -> Vec<PageInfo> {
    indices
        .iter()
        .filter_map(|i| doc.pages.iter().find(|p| p.index == *i).cloned())
        .collect()
}

/// 批量结果摘要（parse_batch / translate_batch 共用）
fn outcome(doc: &BindDoc, updated: Vec<PageInfo>) -> ParseOutcome {
    ParseOutcome {
        book_status: doc.status,
        updated_pages: updated,
    }
}

/// 解析一批页（OCR）：读索引定位绑定 JSON → POST /ocr/pages（token 头）→
/// 块映射 + 页 patch → 状态迁移 → 整批一次原子写。翻译由前端随后调 translate_pdf
/// 并发执行（用户拍板 2026-09-14：pipeline 不再等翻译，翻页隔页并发见 translate_batch）
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

    // 快速失败：书/绑定 JSON 不存在就没必要跑模型
    {
        let lock = file_lock(root, id);
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let _ = load_bind(root, id)?;
    }

    // 批量推理：reqwest 默认无总超时——引擎懒加载时首个请求以分钟计
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

    // 锁内落盘：重读（合并翻译批次同时写入的译文）→ patch → 原子写
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

/// 翻译相位分组（隔页并发）：位置 0,2,4… 一相位、1,3,5… 一相位
fn phase_pages(indices: &[u32], phase: usize) -> Vec<u32> {
    indices
        .iter()
        .enumerate()
        .filter(|(i, _)| i % 2 == phase)
        .map(|(_, v)| *v)
        .collect()
}

/// 只翻译（不 OCR）指定页：供翻译失败重试（finished && !translated）。
/// 隔页并发（用户拍板 2026-09-14）：位置 0,2,4… 一个相位、1,3,5… 一个相位——
/// 相位内页互不相邻，上下文候选不会互踩；相位间串行，后相位可见前相位落盘的
/// external 绑定。页级 catch，全部无进展则不写盘（前端按无进展记 strike）。
pub async fn translate_batch(
    root: &str,
    id: &str,
    indices: Vec<u32>,
    llm: &LlmConfig,
) -> Result<ParseOutcome, String> {
    // 每页 = 一次付费 LLM 请求（两相位并发跑）：限页数与去重，别让前端的错参数放大成并发风暴
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
    // 翻译被用户关掉（2026-09-15）：不碰网络，原文当译文落盘并标记完成
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

        // 快照（锁内读盘；相位 2 可见相位 1 落盘的 external 绑定）
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

        // 并发翻译（锁外；相位内页互不相邻）
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

        // 锁内落盘：重读 → 应用本相位结果（含 external 邻页）→ 原子写
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

/// 翻译禁用路径（用户 2026-09-15）：不碰网络/提示词，逐页把原文复制成译文并标记完成，
/// 一次原子写。前端调度链与联网路径完全一致（都是 translate_pdf），只有这里分流。
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

/// 打开书补骨架：pages 为空（lopdf 解析失败的书）时按实测页数重建 1..=N 骨架；
/// 已有页一律 no-op（绝不覆盖既有 OCR 数据）。返回当前书状态。
pub async fn prefill_pages(root: &str, id: &str, total: u32) -> Result<PDFStatus, String> {
    let (json_path, mut doc) = load_bind(root, id)?;
    if !doc.pages.is_empty() || total == 0 {
        return Ok(doc.status);
    }
    doc.pages = build_skeleton(total)?;
    write_bind_atomic(&json_path, &doc)?;
    Ok(doc.status)
}

/// 清除一本书的解析状态（用户 2026-09-15）：丢弃全部 OCR 块与译文，按页数重建
/// 1..=N 空骨架并原子写回（PDF 本体不动，译文栏回到未解析态）。
/// total = 前端 pdfjs 实测页数；为 0（书未打开/取不到）时沿用 JSON 里已有的页数。
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

/// 页数上限：正常 PDF 远低于此；畸形/恶意文件（或前端传错）不能让我们分配巨型 Vec——
/// release 下 panic=abort，OOM 会直接杀掉进程
/// 单批页数上限（Rust 侧硬上限：OCR 批与翻译批共用；服务端公布值会被夹到它）
pub(crate) const MAX_BATCH_PAGES: u32 = 32;

/// 导入预填充的页数上限（畸形 PDF 保护）
const MAX_PAGES: u32 = 20_000;

/// 1..=N 空页骨架（导入预填充 / 打开书补骨架共用）
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

/// 单行截断到 ≤cap 个字符（日志/错误摘要；按字符边界切，不 panic）
pub(crate) fn truncate(s: &str, cap: usize) -> &str {
    match s.char_indices().nth(cap) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 清除解析状态（用户 2026-09-15）：整份 JSON 回到 1..=N 空骨架，PDF 不动
    /// 表块网格补齐（用户 2026-09-16）：只对可解析表格落盘，
    /// 解析不了的保持 grid=None（前端据此不把它当补翻目标）
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
        // 有内容、已翻译、状态 Finished 的绑定 JSON
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

        // total = 0 → 沿用 JSON 里已有页数
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
        // image → content 空串（figure 契约），translation None（bypass 阶段）
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
        assert!(!doc.pages[1].translated); // 新 OCR 的页需重翻
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

        // Processing 保持
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
        // 上限：畸形页数一律拒绝，不分配巨型 Vec
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
