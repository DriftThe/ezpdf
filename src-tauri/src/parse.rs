//! OCR 批量解析（阶段4）：前端离屏渲染页图 → 批量调 pyserver /ocr/pages →
//! px→pt 映射 → 整批一次原子写回绑定 JSON。
//!
//! 批次语义（用户拍板）：parse_append 每批 ≤4 页、同一本书；本模块每批恰好
//! 一次读 JSON → 一次批量推理 → 一次 tmp+rename 原子写——崩溃最多丢一批
//! （≤4 页），JSON 恒为完整一致快照，永不半写。

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{read_index, BindDoc, Block, PageInfo, PDFStatus};

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
    pub pdf_id: String,
    pub book_status: PDFStatus,
    pub finished_pages: u32,
    pub total_pages: u32,
    pub updated_pages: Vec<PageInfo>,
}

// ---- pyserver /ocr/pages 响应（只取需要的字段）----

#[derive(Deserialize)]
struct OcrBatchResponse {
    pages: Vec<OcrPageResponse>,
}

#[derive(Deserialize)]
struct OcrPageResponse {
    #[serde(default)]
    #[allow(dead_code)]
    width: u32,
    #[serde(default)]
    #[allow(dead_code)]
    height: u32,
    blocks: Vec<OcrBlockResponse>,
}

#[derive(Deserialize)]
struct OcrBlockResponse {
    label: String,
    #[serde(default)]
    #[allow(dead_code)]
    score: f64,
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
            loc: [
                px_to_pt(b.bbox_px[0], scale),
                px_to_pt(b.bbox_px[1], scale),
                px_to_pt(b.bbox_px[2], scale),
                px_to_pt(b.bbox_px[3], scale),
            ],
            translation: None,
        })
        .collect()
}

/// 把一批 (index, blocks) patch 进 BindDoc：整批 finished=true；
/// 索引不在骨架内的页静默跳过；返回实际更新页的克隆（供前端增量渲染）
fn patch_pages(doc: &mut BindDoc, updates: Vec<(u32, Vec<Block>)>) -> Vec<PageInfo> {
    let mut updated = Vec::new();
    for (index, blocks) in updates {
        if let Some(page) = doc.pages.iter_mut().find(|p| p.index == index) {
            page.blocks = blocks;
            page.finished = true;
            updated.push(page.clone());
        }
    }
    updated
}

/// 书级状态迁移（Rust 单写者职责）：
/// - 全页 finished（且至少一页）→ Finished
/// - 尚未开始（Pending）→ Processing（批间残留状态；崩溃后重启据此续跑）
/// - 已是 Processing/Finished → 保持
fn finalize_status(doc: &mut BindDoc) {
    doc.status = if !doc.pages.is_empty() && doc.pages.iter().all(|p| p.finished) {
        PDFStatus::Finished
    } else if matches!(doc.status, PDFStatus::Pending) {
        PDFStatus::Processing
    } else {
        return;
    };
}

/// 原子写绑定 JSON：tmp + rename（Windows fs::rename = MOVEFILE_REPLACE_EXISTING，
/// 覆盖已存在目标）
fn write_bind_atomic(json_path: &Path, doc: &BindDoc) -> Result<(), String> {
    let text = serde_json::to_string_pretty(doc)
        .map_err(|e| format!("Failed when serializing bound JSON: {e}"))?;
    let tmp = json_path.with_extension("json.tmp");
    fs::write(&tmp, text).map_err(|e| format!("写入临时文件失败: {e}"))?;
    fs::rename(&tmp, json_path).map_err(|e| format!("原子替换绑定 JSON 失败: {e}"))
}

/// 解析一批页：读索引定位绑定 JSON → 读 BindDoc → POST /ocr/pages（token 头）→
/// 块映射 + 页 patch → 状态迁移 → 整批一次原子写 → 进度摘要
pub async fn parse_batch(
    root: &str,
    id: &str,
    pages: Vec<ParsePageInput>,
    base: &str,
    token: &str,
) -> Result<ParseOutcome, String> {
    if pages.is_empty() {
        return Err("OCR 批次为空".into());
    }
    if pages.len() > 32 {
        return Err(format!("单批页数超上限: {} > 32", pages.len()));
    }

    let dir = Path::new(root);
    let entry = {
        let index = read_index(root)?;
        index
            .pdfs
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("PDF id not found in repo: {id}"))?
    };
    let bind = entry
        .bind
        .as_ref()
        .ok_or_else(|| format!("PDF 未绑定结构 JSON: {id}"))?;
    let json_path = dir.join(bind);
    let text = fs::read_to_string(&json_path)
        .map_err(|e| format!("Failed when reading bound JSON: {e}"))?;
    let mut doc: BindDoc = serde_json::from_str(&text)
        .map_err(|e| format!("Failed when parsing bound JSON: {e}"))?;

    // 批量推理：reqwest 默认无总超时——引擎懒加载时首个请求以分钟计
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "pages": pages
            .iter()
            .map(|p| serde_json::json!({ "image_b64": p.image_b64, "scale": p.scale }))
            .collect::<Vec<_>>()
    });
    let resp = client
        .post(format!("{base}/ocr/pages"))
        .header("x-ezpdf-token", token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("OCR 服务请求失败: {e}"))?;
    let http = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("OCR 服务响应读取失败: {e}"))?;
    if !http.is_success() {
        return Err(format!("OCR 服务返回 {http}: {}", truncate(&text, 300)));
    }
    let ocr: OcrBatchResponse = serde_json::from_str(&text)
        .map_err(|e| format!("OCR 服务响应解析失败: {e}"))?;
    if ocr.pages.len() != pages.len() {
        return Err(format!(
            "OCR 返回页数不匹配: 请求 {} 返回 {}",
            pages.len(),
            ocr.pages.len()
        ));
    }

    let updates: Vec<(u32, Vec<Block>)> = pages
        .iter()
        .zip(&ocr.pages)
        .map(|(p, r)| (p.index, map_blocks(&r.blocks, p.scale)))
        .collect();
    let updated = patch_pages(&mut doc, updates);
    finalize_status(&mut doc);
    write_bind_atomic(&json_path, &doc)?;

    let finished_pages = doc.pages.iter().filter(|p| p.finished).count() as u32;
    Ok(ParseOutcome {
        pdf_id: id.to_string(),
        book_status: doc.status,
        finished_pages,
        total_pages: doc.pages.len() as u32,
        updated_pages: updated,
    })
}

/// 打开书补骨架：pages 为空（lopdf 解析失败的书）时按实测页数重建 1..=N 骨架；
/// 已有页一律 no-op（绝不覆盖既有 OCR 数据）。返回当前书状态。
pub async fn prefill_pages(root: &str, id: &str, total: u32) -> Result<PDFStatus, String> {
    let entry = {
        let index = read_index(root)?;
        index
            .pdfs
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| format!("PDF id not found in repo: {id}"))?
    };
    let bind = entry
        .bind
        .as_ref()
        .ok_or_else(|| format!("PDF 未绑定结构 JSON: {id}"))?;
    let json_path = Path::new(root).join(bind);
    let text = fs::read_to_string(&json_path)
        .map_err(|e| format!("Failed when reading bound JSON: {e}"))?;
    let mut doc: BindDoc = serde_json::from_str(&text)
        .map_err(|e| format!("Failed when parsing bound JSON: {e}"))?;
    if !doc.pages.is_empty() || total == 0 {
        return Ok(doc.status);
    }
    doc.pages = build_skeleton(total);
    write_bind_atomic(&json_path, &doc)?;
    Ok(doc.status)
}

fn build_skeleton(total: u32) -> Vec<PageInfo> {
    (1..=total)
        .map(|i| PageInfo {
            index: i,
            finished: false,
            blocks: Vec::new(),
        })
        .collect()
}

fn truncate(s: &str, cap: usize) -> &str {
    match s.char_indices().nth(cap) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ocr_block(label: &str, bbox_px: [f64; 4], markdown: &str) -> OcrBlockResponse {
        OcrBlockResponse {
            label: label.into(),
            score: 0.9,
            bbox_px,
            markdown: markdown.into(),
        }
    }

    fn page(index: u32, finished: bool) -> PageInfo {
        PageInfo {
            index,
            finished,
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
        };
        let updated = patch_pages(
            &mut doc,
            vec![(2, vec![blk("b")]), (99, vec![])],
        );
        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0].index, 2);
        assert!(!doc.pages[0].finished);
        assert!(doc.pages[1].finished);
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
        let pages = build_skeleton(3);
        assert_eq!(pages.iter().map(|p| p.index).collect::<Vec<_>>(), vec![1, 2, 3]);
        assert!(pages.iter().all(|p| !p.finished && p.blocks.is_empty()));
    }
}
