//! Mistral-shaped OCR client (`POST /v1/ocr`): the format other OCR services speak.
//!
//! One request carries a PDF as a base64 `document_url` and the answer is converted into the bound-JSON
//! `Block`s the reader already understands. The protocol cannot express "pages 20-23" — `pages` is a
//! selector, the whole document still travels — so the batch is uploaded as a *slice* (`payload_for`),
//! which is what keeps a 622-page book from being re-uploaded in full per batch. Slicing is best-effort;
//! a file it cannot rewrite faithfully travels whole with a `pages` filter, and the two paths number
//! their answers differently, which `collect_updates` resolves from the reply itself.
//!
//! Four things need care:
//!
//! * **geometry** — the service reports boxes in the pixels of *its* render, so they are mapped onto the
//!   viewer's page by the `dimensions` width/height ratio and never by dpi (`dpi` is null for image
//!   documents, and a deployment may render at any scale). The page's own pt size comes from the
//!   frontend's pdfjs viewport, which is what the covers divide by; a page whose aspect ratio disagrees
//!   with the viewer is refused rather than covered with misplaced boxes.
//! * **labels** — `type` is Mistral's 13-class vocabulary. Our own service also sends its PP-DocLayout
//!   label in an extra field, which passes through untouched; anything else is mapped (Mistral and
//!   DeepSeek-OCR names → the client's 21 classes, unknown → `text`).
//! * **cleanup** — a cloud service merges nothing, so the near-duplicate/junk rules that pyserver's
//!   `boxes.py` applies to its own boxes are ported here (see `dedup` below).
//! * **failure** — a page the client cannot place is refused on its own (`Refusal`) so its batchmates
//!   still land; only batch-level errors propagate as `Err`. Retryable replies are retried here, and an
//!   exhausted 429 comes back with the `RATE_LIMITED` marker so the scheduler backs off globally
//!   instead of counting a strike.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde::Deserialize;
use serde_json::json;
use ts_rs::TS;

use crate::table::TABLE;
use crate::translate::log;
use crate::Block;

/// Sent unless the user names another model. A LiteLLM deployment or a gateway picks the upstream from
/// this field, so it is a setting rather than a constant.
pub const DEFAULT_MODEL: &str = "mistral-ocr-latest";

/// Total budget for one batch: the service renders and recognizes every page it was asked for, which on
/// a loaded deployment is minutes rather than seconds.
const REQUEST_TIMEOUT_SECS: u64 = 300;
/// Attempts for a retryable failure (429, 5xx, timeout): first try plus two retries.
const MAX_ATTEMPTS: u32 = 3;
/// Longest `Retry-After` worth waiting out inside one request; beyond it the batch gives up and the
/// scheduler parks instead, so a queue of batches does not each sit on a long sleep.
const MAX_INLINE_BACKOFF_SECS: f64 = 30.0;
/// Prefix on the error of an exhausted 429. Deliberately a string marker rather than a structured error:
/// commands return `Result<_, String>`, and the frontend only needs to tell "we are over quota" from
/// "this batch is broken" (see the cooldown in `stores/parse.ts`).
pub const RATE_LIMITED: &str = "rate-limited: ";

/// Page aspect-ratio tolerance: a larger disagreement means the service rendered a different page box
/// (rotation, a different crop) and the boxes cannot be placed.
const ASPECT_TOLERANCE: f64 = 0.02;
/// Below this the disagreement is only worth a log line, not a refusal.
const ASPECT_NOTICE: f64 = 0.005;
/// A box thinner than this in pt is a degenerate detection, not a block.
const MIN_BOX_PT: f64 = 0.5;
/// Frame comparison tolerance when verifying what slicing produced (pt).
const FRAME_EPSILON: f64 = 0.01;

/// One requested page: the index (1-based, matching the bound JSON) and the viewer's page size in pt.
#[derive(Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct MistralPageInput {
    pub index: u32,
    pub size_pt: [f64; 2],
}

#[derive(Deserialize)]
pub struct OcrResponse {
    pub pages: Vec<OcrPage>,
}

#[derive(Deserialize, Clone)]
pub struct OcrPage {
    pub index: u32,
    #[serde(default)]
    dimensions: Option<Dimensions>,
    #[serde(default)]
    blocks: Option<Vec<OcrBlock>>,
}

/// The service's render size. `dpi` is deliberately not read: it is null for image documents and a
/// deployment may render at any scale, so boxes are placed by the width/height ratio instead.
#[derive(Deserialize, Clone)]
struct Dimensions {
    width: f64,
    height: f64,
}

#[derive(Deserialize, Clone)]
struct OcrBlock {
    #[serde(rename = "type")]
    kind: String,
    top_left_x: f64,
    top_left_y: f64,
    bottom_right_x: f64,
    bottom_right_y: f64,
    #[serde(default)]
    content: String,
    /// Our own service's PP-DocLayout label (an extra field on a Mistral block).
    #[serde(default)]
    label: Option<String>,
}

// ---- payload: the pages that travel, and how the answer will be numbered ----

/// Where, with what secret, and under which model name a batch is sent.
#[derive(Clone, Debug)]
pub struct OcrClient {
    pub base: String,
    pub token: String,
    pub model: String,
}

impl OcrClient {
    pub fn new(base: &str, token: &str, model: Option<&str>) -> Self {
        let model = model.map(str::trim).filter(|m| !m.is_empty()).unwrap_or(DEFAULT_MODEL);
        Self {
            base: base.to_string(),
            token: token.to_string(),
            model: model.to_string(),
        }
    }
}

/// What actually travels: the requested pages alone, or the whole file with a `pages` filter. The two
/// number their answers differently, which is why the choice cannot stay inside the request builder.
pub enum Payload {
    /// Only the requested pages are in the document, so the service counts them 0..n-1 in the order we
    /// asked. This is the normal path — a 25 MB book re-uploaded per batch is the cost it avoids.
    Slice(String),
    /// Slicing was not possible (encrypted, unreadable, or the result did not verify), so the whole file
    /// travels and `pages` selects. The service then numbers pages by their absolute document index.
    Whole(String),
}

impl Payload {
    fn base64(&self) -> &str {
        match self {
            Payload::Slice(encoded) | Payload::Whole(encoded) => encoded,
        }
    }
}

/// The PDF to upload for `pages` (1-based, client space), sliced when lopdf can do it safely.
pub fn payload_for(root: &str, id: &str, pages: &[u32]) -> Result<Payload, String> {
    match parsed_pdf(root, id) {
        Ok(doc) => match slice(&doc, pages) {
            Ok(encoded) => return Ok(Payload::Slice(encoded)),
            Err(reason) => {
                // Slicing is an optimisation: any doubt falls back to the document itself, and the reason
                // is always logged — a silent fallback would quietly triple the upload.
                crate::translate::log(format!("[ocr] PDF slice unavailable, sending the whole file: {reason}"));
            }
        },
        Err(reason) => crate::translate::log(format!("[ocr] PDF not sliceable, sending the whole file: {reason}")),
    }
    let path = book_pdf_path(root, id)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("failed to read PDF {}: {e}", path.display()))?;
    Ok(Payload::Whole(base64_encode(&bytes)))
}

fn book_pdf_path(root: &str, id: &str) -> Result<std::path::PathBuf, String> {
    let entry = crate::find_pdf(root, id)?;
    let name = crate::pdf_file_name(&entry.name, &entry.id);
    crate::check_relative(&name)?;
    Ok(std::path::Path::new(root).join(&name))
}

/// The book's PDF, parsed — cached one at a time keyed by size + mtime, because parsing a real book costs
/// seconds (minutes in a debug build) and every batch needs the same document sliced differently. Each
/// batch clones it, so the cache itself is never mutated.
fn parsed_pdf(root: &str, id: &str) -> Result<Arc<lopdf::Document>, String> {
    let path = book_pdf_path(root, id)?;
    let meta = std::fs::metadata(&path).map_err(|e| format!("failed to read PDF {}: {e}", path.display()))?;
    let stamp = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let key = format!("{}|{}|{}|{}", root, id, meta.len(), stamp);

    static CACHE: OnceLock<Mutex<Option<(String, Arc<lopdf::Document>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    {
        let guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((cached_key, doc)) = guard.as_ref() {
            if *cached_key == key {
                return Ok(doc.clone());
            }
        }
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("failed to read PDF {}: {e}", path.display()))?;
    let doc = Arc::new(lopdf::Document::load_mem(&bytes).map_err(|e| format!("unreadable PDF: {e}"))?);
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    *guard = Some((key, doc.clone()));
    Ok(doc)
}

/// Extract `pages` into a standalone PDF; `Err` means "not worth trusting, send the whole document
/// instead" and carries the reason for the log.
///
/// The page tree is **flattened**, not edited page by page: the root `Pages` node's `Kids` is replaced
/// with the wanted pages and each of those gets its inherited attributes pulled down and its `Parent`
/// re-pointed, after which one reachability pass drops everything else (`delete_pages` would instead walk
/// the whole object table once per removed page — hundreds of passes on a real book, minutes of work).
/// Anything that could notice the rewrite — outlines, named destinations, form fields — is dropped from
/// the catalog first, because a reader that resolves a dangling page reference could refuse to render.
fn slice(cached: &lopdf::Document, pages: &[u32]) -> Result<String, String> {
    if pages.is_empty() {
        return Err("no pages requested".to_string());
    }
    // The cached document is the book's; each batch shapes its own copy.
    let mut doc = cached.clone();
    // An encrypted file would save its streams still encrypted with the /Encrypt dictionary dropped,
    // which renders as blank pages instead of failing — the worst kind of wrong answer.
    if doc.trailer.has(b"Encrypt") {
        return Err("the document is encrypted".to_string());
    }
    let total = doc.get_pages().len() as u32;
    if let Some(bad) = pages.iter().find(|page| **page == 0 || **page > total) {
        return Err(format!("page {bad} does not exist in a {total}-page document"));
    }

    // Ascending document order, which is what `collect_updates` maps the answer's 0..n-1 onto.
    let mut wanted: Vec<u32> = pages.to_vec();
    wanted.sort_unstable();
    wanted.dedup();

    let mut frames: Vec<PageFrame> = Vec::with_capacity(wanted.len());
    for page_no in &wanted {
        frames.push(page_frame(&doc, *page_no).ok_or_else(|| format!("page {page_no} has no readable box"))?);
    }
    let pages_obj = doc
        .catalog()
        .and_then(|catalog| catalog.get(b"Pages"))
        .and_then(lopdf::Object::as_reference)
        .map_err(|_| "the document has no page tree".to_string())?;
    let kept: Vec<lopdf::ObjectId> = wanted.iter().map(|p| doc.get_pages()[p]).collect();

    for &page_id in &kept {
        let inherited = inherited_attributes(&doc, page_id);
        let dict = doc
            .get_dictionary_mut(page_id)
            .map_err(|e| format!("unreadable page object: {e}"))?;
        for (key, value) in inherited {
            dict.set(key, value);
        }
        dict.set("Parent", lopdf::Object::Reference(pages_obj));
    }
    {
        let tree = doc
            .get_dictionary_mut(pages_obj)
            .map_err(|e| format!("unreadable page tree: {e}"))?;
        tree.set(
            "Kids",
            lopdf::Object::Array(kept.iter().map(|id| lopdf::Object::Reference(*id)).collect()),
        );
        tree.set("Count", lopdf::Object::Integer(kept.len() as i64));
    }
    // Metadata a renderer never needs for OCR, and each of which can hold a reference to a page that is
    // about to be pruned — a reader resolving a dangling page reference could refuse to render.
    let catalog = doc.catalog_mut().map_err(|e| format!("unreadable catalog: {e}"))?;
    for key in [
        b"Outlines".as_slice(),
        b"Dests".as_slice(),
        b"Names".as_slice(),
        b"PageLabels".as_slice(),
        b"StructTreeRoot".as_slice(),
        b"AcroForm".as_slice(),
    ] {
        catalog.remove(key);
    }

    // One reachability pass; what it does not reach is another page's content, fonts and images.
    let reachable: std::collections::HashSet<lopdf::ObjectId> = doc.traverse_objects(|_| {}).into_iter().collect();
    let dead: Vec<lopdf::ObjectId> = doc
        .objects
        .keys()
        .copied()
        .filter(|id| !reachable.contains(id))
        .collect();
    for id in dead {
        doc.objects.remove(&id);
    }
    doc.renumber_objects();
    doc.compress();

    let mut out = Vec::new();
    doc.save_to(&mut out).map_err(|e| format!("could not rewrite the PDF: {e}"))?;

    // Verify before trusting it: the same page count, and every page box unchanged. Flattening the tree
    // is the one step here that can quietly change what a renderer sees.
    let check = lopdf::Document::load_mem(&out).map_err(|e| format!("rewritten PDF is unreadable: {e}"))?;
    if check.get_pages().len() as u32 != wanted.len() as u32 {
        return Err(format!(
            "the rewritten document has {} pages instead of {}",
            check.get_pages().len(),
            wanted.len()
        ));
    }
    for (position, expected) in frames.iter().enumerate() {
        let Some(actual) = page_frame(&check, position as u32 + 1) else {
            return Err("a rewritten page has no readable box".to_string());
        };
        let same_box = (actual.width_pt - expected.width_pt).abs() <= FRAME_EPSILON
            && (actual.height_pt - expected.height_pt).abs() <= FRAME_EPSILON;
        if !same_box || actual.rotate != expected.rotate {
            return Err(format!("the rewritten page {} moved its page box", wanted[position]));
        }
    }
    Ok(base64_encode(&out))
}

/// The attributes a page inherits from its ancestors and would lose when the tree is flattened: the
/// effective box, its rotation, and the resources its content stream refers to.
fn inherited_attributes(doc: &lopdf::Document, page_id: lopdf::ObjectId) -> Vec<(&'static [u8], lopdf::Object)> {
    const KEYS: [&[u8]; 4] = [b"MediaBox", b"CropBox", b"Rotate", b"Resources"];
    let mut out = Vec::new();
    let Ok(mut dict) = doc.get_dictionary(page_id) else {
        return out;
    };
    let mut missing: Vec<&'static [u8]> = KEYS
        .iter()
        .copied()
        .filter(|key| dict.get(key).is_err())
        .collect();
    while !missing.is_empty() {
        let Ok(parent) = dict.get(b"Parent").and_then(lopdf::Object::as_reference) else {
            break;
        };
        let Ok(next) = doc.get_dictionary(parent) else {
            break;
        };
        missing.retain(|key| match next.get(*key) {
            Ok(value) => {
                out.push((*key, value.clone()));
                false
            }
            Err(_) => true,
        });
        dict = next;
    }
    out
}

/// The box a renderer would use for a page, plus its rotation — both possibly inherited from the page
/// tree, which is why they are read by walking up rather than from the page dictionary alone.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PageFrame {
    pub width_pt: f64,
    pub height_pt: f64,
    pub rotate: i64,
}

/// `page_no` is 1-based document order, the numbering the bound JSON uses.
pub fn page_frame(doc: &lopdf::Document, page_no: u32) -> Option<PageFrame> {
    let id = *doc.get_pages().get(&page_no)?;
    let mut dict = doc.get_dictionary(id).ok()?;
    let mut rotate = 0;
    let mut box_values: Option<Vec<f64>> = None;
    loop {
        if box_values.is_none() {
            for key in [b"CropBox".as_slice(), b"MediaBox".as_slice()] {
                if let Ok(array) = dict.get(key).and_then(|v| v.as_array()) {
                    let values: Vec<f64> = array.iter().filter_map(number).collect();
                    if values.len() == 4 {
                        box_values = Some(values);
                        break;
                    }
                }
            }
        }
        if rotate == 0 {
            if let Ok(value) = dict.get(b"Rotate").and_then(|v| v.as_i64()) {
                rotate = value;
            }
        }
        let Some(parent) = dict.get(b"Parent").and_then(|v| v.as_reference()).ok() else {
            break;
        };
        let Ok(next) = doc.get_dictionary(parent) else { break };
        dict = next;
    }
    let values = box_values?;
    // Quarters of a full turn: the rotation pdfjs applies to the viewport, which the aspect check needs.
    let rotate = ((rotate % 360) + 360) % 360;
    Some(PageFrame {
        width_pt: (values[2] - values[0]).abs(),
        height_pt: (values[3] - values[1]).abs(),
        rotate,
    })
}

fn number(object: &lopdf::Object) -> Option<f64> {
    match object {
        lopdf::Object::Integer(value) => Some(*value as f64),
        lopdf::Object::Real(value) => Some(*value as f64),
        _ => None,
    }
}

// ---- request ----

/// One batch: `POST {base}/v1/ocr`, retrying the failures that are worth retrying.
pub async fn ocr_pdf(client: &OcrClient, payload: &Payload, pages: &[u32]) -> Result<Vec<OcrPage>, String> {
    let body = request_body(client, payload, pages);
    let http = crate::pyserver::ocr_client(&client.base, Some(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS)))?;
    let url = format!("{}/v1/ocr", client.base);
    let mut message = String::new();
    for attempt in 0..MAX_ATTEMPTS {
        let last = attempt + 1 == MAX_ATTEMPTS;
        let retry_after;
        match http
            .post(&url)
            .header("Authorization", format!("Bearer {}", client.token))
            .json(&body)
            .send()
            .await
        {
            // Includes the request timeout: a half-read answer is no answer.
            Err(err) => {
                message = format!("OCR service request failed: {err}");
                retry_after = None;
            }
            Ok(resp) => {
                let status = resp.status();
                retry_after = retry_after_secs(&resp);
                let text = resp
                    .text()
                    .await
                    .map_err(|e| format!("failed to read OCR service response: {e}"))?;
                if status.is_success() {
                    let parsed: OcrResponse = serde_json::from_str(&text)
                        .map_err(|e| format!("failed to parse OCR service response: {e}"))?;
                    return Ok(parsed.pages);
                }
                message = format!("OCR service returned {status}: {}", error_message(&text));
                match retryable(status.as_u16()) {
                    // Bad key, unknown model, malformed request: retrying cannot change the answer.
                    Retry::Never => return Err(message),
                    // Over quota: the client as a whole is throttled, so a long wait is the caller's
                    // business (it parks globally) rather than this batch sitting on a sleep.
                    Retry::Quota if last || retry_after.unwrap_or(0.0) > MAX_INLINE_BACKOFF_SECS => {
                        return Err(format!("{RATE_LIMITED}{message}"))
                    }
                    Retry::Quota | Retry::Transient => {}
                }
            }
        }
        if last {
            return Err(message);
        }
        backoff(attempt, retry_after).await;
    }
    Err(message)
}

enum Retry {
    /// 429: another identical request may well succeed once the quota window moves.
    Quota,
    /// 5xx and timeouts: the service itself may recover.
    Transient,
    /// Everything else: the answer would be the same.
    Never,
}

fn retryable(status: u16) -> Retry {
    match status {
        429 => Retry::Quota,
        500 | 502 | 503 | 504 => Retry::Transient,
        _ => Retry::Never,
    }
}

fn retry_after_secs(resp: &reqwest::Response) -> Option<f64> {
    resp.headers()
        .get("retry-after")?
        .to_str()
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

async fn backoff(attempt: u32, retry_after: Option<f64>) {
    let wait = retry_after.unwrap_or_else(|| 2f64.powi(attempt as i32));
    tokio::time::sleep(std::time::Duration::from_secs_f64(wait.clamp(0.1, MAX_INLINE_BACKOFF_SECS))).await;
}

/// Mistral's request shape: the PDF as a data URI, `include_blocks` explicit (the spec defaults it to
/// false, so never rely on a default), and the two fields a generic endpoint leaves to chance:
/// tables are asked for as HTML, which is the form `table::parse_html` reads (markdown pipes cannot
/// express `rowspan`), and image crops are switched off because the reader keeps the original pixels
/// and never uses them.
fn request_body(client: &OcrClient, payload: &Payload, pages: &[u32]) -> serde_json::Value {
    let mut body = json!({
        "model": client.model,
        "document": {
            "type": "document_url",
            "document_url": format!("data:application/pdf;base64,{}", payload.base64()),
        },
        "include_blocks": true,
        "include_image_base64": false,
        "table_format": "html",
    });
    // A slice *is* the selection, so the filter would only re-number what we already asked for.
    if let Payload::Whole(_) = payload {
        body["pages"] = json!(pages.iter().map(|p| p.saturating_sub(1)).collect::<Vec<u32>>());
    }
    body
}

/// Error text from any of the error styles in the wild: our `{"detail": …}`, Mistral/OpenAI's
/// `{"object":"error","message":…}` and the nested `{"error":{"message":…}}` a gateway may send.
fn error_message(text: &str) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(detail) = value.get("detail").and_then(|v| v.as_str()) {
            return detail.to_string();
        }
        if let Some(message) = value.get("message").and_then(|v| v.as_str()) {
            return message.to_string();
        }
        if let Some(message) = value.pointer("/error/message").and_then(|v| v.as_str()) {
            return message.to_string();
        }
    }
    crate::parse::truncate(text, 300).to_string()
}

// ---- response → blocks ----

/// One page of the answer → the page's blocks (pt coordinates, client types). Err refuses that page.
///
/// `client_index` is only used to name the page: the answer's own index is local to the uploaded
/// document, which for a slice says nothing about where the user sees the page.
pub fn page_blocks(page: &OcrPage, size_pt: [f64; 2], client_index: u32) -> Result<Vec<Block>, String> {
    let index = client_index;
    let dims = page
        .dimensions
        .as_ref()
        .ok_or_else(|| format!("page {index}: the service reported no dimensions"))?;
    if !(dims.width > 1.0 && dims.height > 1.0) {
        return Err(format!(
            "page {index}: unusable dimensions {}x{}",
            dims.width, dims.height
        ));
    }
    let (page_w, page_h) = (size_pt[0], size_pt[1]);
    if !(page_w > 0.0 && page_h > 0.0) {
        return Err(format!("page {index}: the viewer reported an empty page size"));
    }
    let expected = page_w / page_h;
    let reported = dims.width / dims.height;
    let drift = ((expected - reported) / expected).abs();
    if drift > ASPECT_TOLERANCE {
        return Err(format!(
            "page {index}: the service rendered a different page box (aspect {reported:.3} vs viewer {expected:.3})"
        ));
    }
    if drift > ASPECT_NOTICE {
        log(format!(
            "OCR page {index}: render aspect {reported:.3} differs from the viewer's {expected:.3} — \
             boxes are scaled proportionally",
        ));
    }
    let blocks = page
        .blocks
        .as_ref()
        .ok_or_else(|| format!("page {index}: the service returned no blocks (send include_blocks=true)"))?;

    let (sx, sy) = (page_w / dims.width, page_h / dims.height);
    let mut out: Vec<Block> = Vec::with_capacity(blocks.len());
    for raw in blocks {
        let kind = client_type(raw);
        let content = clean_content(&kind, &raw.content);
        let loc = [
            clamp(raw.top_left_x * sx, 0.0, page_w),
            clamp(raw.top_left_y * sy, 0.0, page_h),
            clamp(raw.bottom_right_x * sx, 0.0, page_w),
            clamp(raw.bottom_right_y * sy, 0.0, page_h),
        ];
        let loc = [round2(loc[0]), round2(loc[1]), round2(loc[2]), round2(loc[3])];
        if loc[2] - loc[0] < MIN_BOX_PT || loc[3] - loc[1] < MIN_BOX_PT {
            continue; // degenerate box: a cover there would be invisible anyway
        }
        let is_image = kind == "image";
        if !is_image && content.is_empty() {
            continue; // no text to translate or show
        }
        let grid = (kind == TABLE).then(|| crate::table::parse_any(&content)).flatten();
        out.push(Block {
            kind,
            content: if is_image { String::new() } else { content },
            loc,
            translation: None,
            grid,
        });
    }
    Ok(dedup(out))
}

/// A page the client could not place. Reported instead of thrown away, and kept out of the batch rather
/// than failing its batchmates: the geometry is the *service's* render, so one rotated page must not
/// cost the other fifteen their turn.
#[derive(Debug)]
pub struct Refusal {
    pub index: u32,
    pub reason: String,
}

/// The answered pages → `(client index, blocks)` per page, plus the pages that had to be refused.
///
/// `Err` stays reserved for a defect of the whole batch (a numbering that cannot be reconciled, an answer
/// that never mentions a requested page); a page that merely cannot be placed comes back in the refusal
/// list. Extra pages are ignored: a service that disregards the `pages` filter and answers the whole
/// document is wasteful, not wrong.
pub fn collect_updates(
    requested: &[MistralPageInput],
    returned: Vec<OcrPage>,
) -> Result<(Vec<(u32, Vec<Block>)>, Vec<Refusal>), String> {
    let mut ascending: Vec<u32> = requested.iter().map(|p| p.index).collect();
    ascending.sort_unstable();
    if ascending.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("OCR batch repeats a page".to_string());
    }
    let mut by_index: HashMap<u32, OcrPage> = HashMap::with_capacity(returned.len());
    for page in returned {
        let index = page.index;
        if by_index.insert(index, page).is_some() {
            return Err(format!("OCR service answered page {index} twice"));
        }
    }

    // Two consistent numberings, and the reply itself says which one this is:
    //   * a slice is a document of its own, so the service counts 0..n-1 in the order we asked;
    //   * an unsliced request answers with absolute document indices, which the protocol counts from 0
    //     while the bound JSON counts from 1.
    // They cannot be confused: a request of exactly 1..n resolves to the same mapping either way.
    let ids: Vec<u32> = {
        let mut ids: Vec<u32> = by_index.keys().copied().collect();
        ids.sort_unstable();
        ids
    };
    let local = ids == (0..requested.len() as u32).collect::<Vec<u32>>();
    if !local && !ascending.iter().all(|index| by_index.contains_key(&(index - 1))) {
        return Err(format!(
            "OCR service answered pages {ids:?}, which do not cover the requested {ascending:?}"
        ));
    }
    let rank: HashMap<u32, u32> = ascending
        .iter()
        .enumerate()
        .map(|(position, index)| (*index, position as u32))
        .collect();

    let mut updates = Vec::with_capacity(requested.len());
    let mut refused = Vec::new();
    for input in requested {
        let key = if local {
            rank[&input.index]
        } else {
            input.index - 1 // the service's page numbers are 0-based, the bound JSON's are not
        };
        let Some(page) = by_index.remove(&key) else {
            return Err(format!("OCR service skipped page {}", input.index));
        };
        match page_blocks(&page, input.size_pt, input.index) {
            Ok(blocks) => updates.push((input.index, blocks)),
            Err(reason) => refused.push(Refusal {
                index: input.index,
                reason,
            }),
        }
    }
    Ok((updates, refused))
}

fn clamp(value: f64, low: f64, high: f64) -> f64 {
    if value.is_nan() {
        return low;
    }
    value.max(low).min(high)
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// The client class for one block: our own label wins, then the Mistral type, then the foreign label.
fn client_type(block: &OcrBlock) -> String {
    if let Some(label) = block.label.as_deref() {
        if is_client_type(label) {
            return label.to_string();
        }
    }
    let mapped = map_mistral_type(&block.kind);
    if let Some(label) = block.label.as_deref() {
        if mapped == "text" {
            // Nothing recognised in `type`; the service's own label may still name a client class
            let from_label = map_mistral_type(label);
            if from_label != "text" {
                return from_label.to_string();
            }
        }
    }
    mapped.to_string()
}

/// The client's own type set: `BLOCK_TYPE_OPTIONS` in src/lib/blocks.ts plus `formula`.
pub fn is_client_type(kind: &str) -> bool {
    matches!(
        kind,
        "text"
            | "paragraph_title"
            | "doc_title"
            | "abstract"
            | "aside_text"
            | "footnote"
            | "footer"
            | "vision_footnote"
            | "figure_title"
            | "content"
            | "reference"
            | "reference_content"
            | "algorithm"
            | "number"
            | "header"
            | "chart"
            | "seal"
            | "table"
            | "image"
            | "formula"
    )
}

/// A foreign block type → a client type. Mistral's 13 types and DeepSeek-OCR's raw grounding labels
/// both land here (that model's own vocabulary is `text`/`title`/`sub_title`/`image`/`image_caption`/
/// `table`/`table_caption`/`equation`, observed over a book's pages). Unknown → `text`.
///
/// The title family is kept inside the title family: `title` is the document-level heading and
/// `sub_title`/`section_header` the section one, which is how the two client types split.
pub fn map_mistral_type(kind: &str) -> &'static str {
    let normalized: String = kind
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c == ' ' || c == '-' { '_' } else { c })
        .collect();
    if is_client_type(&normalized) {
        return match normalized.as_str() {
            "text" => "text",
            "paragraph_title" => "paragraph_title",
            "doc_title" => "doc_title",
            "abstract" => "abstract",
            "aside_text" => "aside_text",
            "footnote" => "footnote",
            "footer" => "footer",
            "vision_footnote" => "vision_footnote",
            "figure_title" => "figure_title",
            "content" => "content",
            "reference" => "reference",
            "reference_content" => "reference_content",
            "algorithm" => "algorithm",
            "number" => "number",
            "header" => "header",
            "chart" => "chart",
            "seal" => "seal",
            "table" => "table",
            "image" => "image",
            _ => "formula",
        };
    }
    match normalized.as_str() {
        "title" | "heading" | "headline" | "doc_title" | "document_title" => "doc_title",
        "sub_title" | "subtitle" | "section_header" | "section_title" => "paragraph_title",
        "caption" | "figure_caption" | "image_caption" | "table_caption" | "figure_title" => {
            "figure_title"
        }
        "equation" | "formula" | "math" | "formula_number" => "formula",
        "table" | "table_body" => "table",
        "image" | "figure" | "picture" | "chart" | "illustration" | "diagram" => "image",
        "code" | "listing" | "algorithm" => "algorithm",
        "reference" | "references" | "bibliography" => "reference",
        "aside_text" | "sidebar" => "aside_text",
        "header" | "page_header" | "running_head" => "header",
        "footer" | "page_footer" => "footer",
        "footnote" | "endnote" => "footnote",
        "abstract" | "summary" | "keywords" => "abstract",
        "number" | "page_number" => "number",
        "seal" | "signature" | "stamp" => "seal",
        _ => "text",
    }
}

/// Text as the reader will show it: markdown heading marks and stray `<center>` wrappers dropped, runs
/// of blank lines collapsed, bare formula LaTeX delimited. Math delimiters are otherwise left alone —
/// the cover renderer already handles `$$…$$`, `\[…\]`, `$…$` and `\(…\)`.
fn clean_content(kind: &str, content: &str) -> String {
    let mut text = content.replace("\r\n", "\n").replace('\r', "\n");
    if matches!(kind, "doc_title" | "paragraph_title" | "figure_title") {
        let trimmed = text.trim_start();
        if trimmed.starts_with('#') {
            text = trimmed.trim_start_matches('#').trim_start().to_string();
        }
        for tag in ["<center>", "</center>"] {
            text = text.replace(tag, "");
        }
    }
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    let text = text.trim().to_string();
    if kind == "formula" {
        normalize_formula(&text)
    } else {
        text
    }
}

/// Any of the four delimiters `src/lib/richText.ts` renders as math: `$$…$$`, `\[…\]`, `$…$`, `\(…\)`.
fn has_math_delimiters(content: &str) -> bool {
    content.contains('$') || ["\\(", "\\)", "\\[", "\\]"].iter().any(|d| content.contains(d))
}

/// A formula block the service sent as bare LaTeX: the cover only runs KaTeX for a delimited expression,
/// so an undelimited reading would be painted as plain text. The wrap is guarded by a LaTeX signal,
/// because a formula block misclassified around prose must stay readable prose.
fn normalize_formula(content: &str) -> String {
    let latex = ['\\', '^', '_', '{', '}', '='].iter().any(|signal| content.contains(*signal));
    if content.is_empty() || has_math_delimiters(content) || !latex {
        return content.to_string();
    }
    format!("$${content}$$")
}

// ---- cleanup ported from pyserver/app/services/boxes.py (a cloud service merges nothing) ----

/// Text-family classes: the ones that carry the page's words, and so the ones the junk rule and the
/// near-duplicate merge apply to (mirrors `TEXT_LABELS` in boxes.py, in client-type spelling;
/// `formula_number` cannot appear here because the label map folds it into `formula`).
const TEXT_FAMILY: &[&str] = &[
    "text",
    "paragraph_title",
    "doc_title",
    "abstract",
    "aside_text",
    "footnote",
    "footer",
    "vision_footnote",
    "figure_title",
    "content",
    "reference",
    "reference_content",
    "number",
];

/// Duplicate merge thresholds — the same numbers as `EZPDF_OCR_DEDUP_TEXT_*` in pyserver/app/config.py.
const DEDUP_TEXT_MIN_OVERLAP: f64 = 0.15;
const DEDUP_TEXT_RATIO: f64 = 0.82;
const DEDUP_TEXT_TOKEN_CONTAINMENT: f64 = 0.7;
const DEDUP_TEXT_FUZZY_PREFIX: usize = 4;
const DEDUP_MIN_CONTENT_CHARS: usize = 2;
const MIN_CONTAINMENT_CHARS: usize = 8;
const MIN_TOKEN_COVERAGE: f64 = 0.6;
const PLACEHOLDERS: [&str; 3] = ["[unlabeled]", "[n/a]", "[no text]"];

fn is_text_family(kind: &str) -> bool {
    TEXT_FAMILY.contains(&kind)
}

/// Drop junk readings, then fold near-duplicate boxes into their most complete copy.
fn dedup(blocks: Vec<Block>) -> Vec<Block> {
    let mut kept: Vec<Option<Block>> = blocks
        .into_iter()
        .map(|block| {
            if is_text_family(&block.kind) && !has_words(&block.content) {
                None // punctuation, a stray glyph or the recognizer's "nothing here" placeholder
            } else {
                Some(block)
            }
        })
        .collect();

    let mut order: Vec<usize> = (0..kept.len())
        .filter(|&i| kept[i].as_ref().is_some_and(|b| is_text_family(&b.kind)))
        .collect();
    // Most complete reading first, so a truncated copy never replaces the full one (score is absent
    // from this protocol; area breaks ties).
    order.sort_by(|&a, &b| {
        let (ba, bb) = (kept[a].as_ref().unwrap(), kept[b].as_ref().unwrap());
        normalized(&bb.content)
            .chars()
            .count()
            .cmp(&normalized(&ba.content).chars().count())
            .then(area(&bb.loc).partial_cmp(&area(&ba.loc)).unwrap_or(std::cmp::Ordering::Equal))
    });

    let mut absorbed = vec![false; kept.len()];
    for (position, &i) in order.iter().enumerate() {
        if absorbed[i] {
            continue;
        }
        for &j in &order[position + 1..] {
            if absorbed[j] {
                continue;
            }
            let (overlap, dual) = {
                let (Some(a), Some(b)) = (kept[i].as_ref(), kept[j].as_ref()) else {
                    continue;
                };
                (iom(&a.loc, &b.loc), texts_look_dual(&a.content, &b.content))
            };
            if overlap < DEDUP_TEXT_MIN_OVERLAP || !dual {
                continue;
            }
            absorbed[j] = true;
            let absorbed_loc = kept[j].as_ref().map(|b| b.loc).unwrap_or([0.0; 4]);
            if let Some(survivor) = kept[i].as_mut() {
                survivor.loc = union(&survivor.loc, &absorbed_loc); // no pixels leave the survivor
            }
        }
    }

    kept.into_iter()
        .enumerate()
        .filter(|(i, block)| !absorbed[*i] && block.is_some())
        .map(|(_, block)| block.unwrap())
        .collect()
}

fn area(loc: &[f64; 4]) -> f64 {
    (loc[2] - loc[0]).max(0.0) * (loc[3] - loc[1]).max(0.0)
}

/// Intersection over the smaller box — "how much of the small box is inside the big one".
fn iom(a: &[f64; 4], b: &[f64; 4]) -> f64 {
    let smaller = area(a).min(area(b));
    if smaller <= 0.0 {
        return 0.0;
    }
    let overlap =
        (a[2].min(b[2]) - a[0].max(b[0])).max(0.0) * (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    overlap / smaller
}

fn union(a: &[f64; 4], b: &[f64; 4]) -> [f64; 4] {
    [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]
}

/// False for a box whose reading found nothing but punctuation or a stray glyph.
fn has_words(content: &str) -> bool {
    let letters = content.chars().filter(|c| c.is_alphanumeric()).count();
    letters >= DEDUP_MIN_CONTENT_CHARS
        && !PLACEHOLDERS
            .iter()
            .any(|p| normalized(content) == normalized(p))
}

/// Casefolded alphanumeric projection (CJK included), the space both text comparisons work in.
fn normalized(content: &str) -> String {
    content
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Word set of a spaced script (CJK has no separators and yields nothing here — the coverage floor in
/// `texts_look_dual` is what keeps such a paragraph from looking like a duplicate of its neighbour).
fn tokens(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for c in content.chars() {
        if c.is_ascii_alphanumeric() {
            current.push(c.to_ascii_lowercase());
        } else if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// OCR truncates a word's tail more often than it invents one ("connota"/"connotation").
fn fuzzy_hit(token: &str, others: &[String]) -> bool {
    if others.iter().any(|other| other == token) {
        return true;
    }
    if DEDUP_TEXT_FUZZY_PREFIX == 0 || token.chars().count() < DEDUP_TEXT_FUZZY_PREFIX {
        return false;
    }
    let head: String = token.chars().take(DEDUP_TEXT_FUZZY_PREFIX).collect();
    others.iter().any(|other| {
        other.chars().count() >= DEDUP_TEXT_FUZZY_PREFIX
            && other.chars().take(DEDUP_TEXT_FUZZY_PREFIX).collect::<String>() == head
    })
}

/// Are these two readings the same text? Deliberately conservative: different text keeps both boxes,
/// because a wrongly dropped block costs the reader a translation while a leftover duplicate only
/// costs ink. Mirrors `texts_look_dual` in boxes.py; the final fallback is a character-bigram Dice
/// coefficient in place of difflib's ratio (same 0.82 bar, no Python runtime here).
fn texts_look_dual(a: &str, b: &str) -> bool {
    let na = normalized(a);
    let nb = normalized(b);
    if na.is_empty() || nb.is_empty() {
        return false;
    }
    if na == nb {
        return true;
    }
    let (short, long) = if na.chars().count() <= nb.chars().count() {
        (na.as_str(), nb.as_str())
    } else {
        (nb.as_str(), na.as_str())
    };
    if short.chars().count() >= MIN_CONTAINMENT_CHARS && long.contains(short) {
        return true;
    }
    let ta = tokens(a);
    let tb = tokens(b);
    if !ta.is_empty() && !tb.is_empty() {
        let (smaller, larger) = if ta.len() <= tb.len() { (&ta, &tb) } else { (&tb, &ta) };
        let shared = smaller.iter().filter(|t| fuzzy_hit(t, larger)).count() as f64 / smaller.len() as f64;
        let coverage = f64::min(
            ta.iter().map(|t| t.chars().count()).sum::<usize>() as f64 / na.chars().count() as f64,
            tb.iter().map(|t| t.chars().count()).sum::<usize>() as f64 / nb.chars().count() as f64,
        );
        let small_tokens = ta.len().min(tb.len());
        if shared >= DEDUP_TEXT_TOKEN_CONTAINMENT
            && coverage >= MIN_TOKEN_COVERAGE
            && (small_tokens >= 3 || (shared == 1.0 && short.chars().count() >= MIN_CONTAINMENT_CHARS))
        {
            return true;
        }
    }
    bigram_dice(&na, &nb) >= DEDUP_TEXT_RATIO
}

/// Sørensen–Dice over character bigrams, the stand-in for difflib's ratio.
fn bigram_dice(a: &str, b: &str) -> f64 {
    let grams = |text: &str| -> HashMap<(char, char), usize> {
        let chars: Vec<char> = text.chars().collect();
        let mut map = HashMap::new();
        for pair in chars.windows(2) {
            *map.entry((pair[0], pair[1])).or_insert(0) += 1;
        }
        map
    };
    let ga = grams(a);
    let gb = grams(b);
    let total: usize = ga.values().sum::<usize>() + gb.values().sum::<usize>();
    if total == 0 {
        return if a == b { 1.0 } else { 0.0 };
    }
    let shared: usize = ga
        .iter()
        .map(|(gram, count)| count.min(gb.get(gram).unwrap_or(&0)))
        .sum();
    2.0 * shared as f64 / total as f64
}

// ---- base64 (the payload is one big data URI; no dependency needed for one encoder) ----

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(triple >> 18) as usize & 0x3f] as char);
        out.push(B64[(triple >> 12) as usize & 0x3f] as char);
        out.push(if chunk.len() > 1 { B64[(triple >> 6) as usize & 0x3f] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[triple as usize & 0x3f] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ocr_block(kind: &str, loc: [f64; 4], content: &str) -> OcrBlock {
        OcrBlock {
            kind: kind.into(),
            top_left_x: loc[0],
            top_left_y: loc[1],
            bottom_right_x: loc[2],
            bottom_right_y: loc[3],
            content: content.into(),
            label: None,
        }
    }

    fn page_with(blocks: Vec<OcrBlock>, width: f64, height: f64) -> OcrPage {
        OcrPage {
            index: 0,
            dimensions: Some(Dimensions { width, height }),
            blocks: Some(blocks),
        }
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"hello world"), "aGVsbG8gd29ybGQ=");
    }

    #[test]
    fn coordinates_scale_by_the_reported_ratio() {
        // A 1000x2000 render of a viewer page of 500x1000 pt: exactly half.
        let page = page_with(vec![ocr_block("text", [100.0, 200.0, 500.0, 400.0], "hello")], 1000.0, 2000.0);
        let blocks = page_blocks(&page, [500.0, 1000.0], 1).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].loc, [50.0, 100.0, 250.0, 200.0]);
        assert_eq!(blocks[0].kind, "text");
        assert_eq!(blocks[0].content, "hello");

        // A differently scaled render of the same page: the ratio, not the dpi, is what places boxes.
        let page = page_with(vec![ocr_block("text", [250.0, 500.0, 1250.0, 1000.0], "hi")], 2000.0, 4000.0);
        let blocks = page_blocks(&page, [500.0, 1000.0], 1).unwrap();
        assert_eq!(blocks[0].loc, [62.5, 125.0, 312.5, 250.0]);
    }

    #[test]
    fn aspect_mismatch_fails_the_page() {
        // Rotation: the service rendered 2000x1000 where the viewer has 500x1000.
        let page = page_with(vec![ocr_block("text", [0.0, 0.0, 10.0, 10.0], "x")], 2000.0, 1000.0);
        let err = page_blocks(&page, [500.0, 1000.0], 1).unwrap_err();
        assert!(err.contains("different page box"), "{err}");
    }

    #[test]
    fn missing_dimensions_or_blocks_fail_the_page() {
        let page = OcrPage { index: 3, dimensions: None, blocks: Some(vec![]) };
        assert!(page_blocks(&page, [500.0, 700.0], 3).unwrap_err().contains("no dimensions"));

        let page = OcrPage {
            index: 4,
            dimensions: Some(Dimensions { width: 100.0, height: 100.0 }),
            blocks: None,
        };
        assert!(page_blocks(&page, [100.0, 100.0], 4).unwrap_err().contains("include_blocks"));
    }

    #[test]
    fn degenerate_and_empty_blocks_are_dropped() {
        let page = page_with(
            vec![
                ocr_block("text", [10.0, 10.0, 10.2, 40.0], "sliver"), // thinner than MIN_BOX_PT
                ocr_block("text", [10.0, 10.0, 40.0, 40.0], "   "),    // no text at all
                ocr_block("text", [10.0, 10.0, 40.0, 40.0], "kept"),
            ],
            100.0,
            100.0,
        );
        let blocks = page_blocks(&page, [100.0, 100.0], 1).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].content, "kept");
    }

    #[test]
    fn image_blocks_empty_their_content_and_tables_get_a_grid() {
        let page = page_with(
            vec![
                ocr_block("image", [0.0, 0.0, 100.0, 100.0], "figure"),
                ocr_block("table", [0.0, 0.0, 100.0, 100.0], "<table><tr><td>a</td><td>b</td></tr></table>"),
            ],
            100.0,
            100.0,
        );
        let blocks = page_blocks(&page, [100.0, 100.0], 1).unwrap();
        assert_eq!(blocks[0].kind, "image");
        assert!(blocks[0].content.is_empty());
        assert_eq!(blocks[1].kind, "table");
        let grid = blocks[1].grid.as_ref().expect("HTML table must parse into a grid");
        assert_eq!(grid.cols, 2);
        assert_eq!(grid.rows[0].cells.len(), 2);
    }

    #[test]
    fn our_own_label_survives_and_foreign_types_are_mapped() {
        let mut own = ocr_block("text", [0.0, 0.0, 10.0, 10.0], "shadowed");
        own.label = Some("footer".into()); // our service: Mistral type + the PP-DocLayout label
        let page = page_with(vec![own], 100.0, 100.0);
        assert_eq!(page_blocks(&page, [100.0, 100.0], 1).unwrap()[0].kind, "footer");

        let cases = [
            ("title", "doc_title"),
            ("sub_title", "paragraph_title"),
            ("image_caption", "figure_title"),
            ("table_caption", "figure_title"),
            ("equation", "formula"),
            ("figure", "image"),
            ("code", "algorithm"),
            ("references", "reference"),
            ("list", "text"),
            ("weird_thing", "text"),
        ];
        for (incoming, expected) in cases {
            assert_eq!(map_mistral_type(incoming), expected, "{incoming}");
        }
    }

    #[test]
    fn heading_marks_and_center_wrappers_are_cleaned() {
        assert_eq!(clean_content("doc_title", "# Student Manual  "), "Student Manual");
        assert_eq!(clean_content("paragraph_title", "## Topics:"), "Topics:");
        assert_eq!(
            clean_content("figure_title", "<center>Figure N1.20: DVM reading</center>"),
            "Figure N1.20: DVM reading"
        );
        // Inline math is left alone: the cover renderer understands these delimiters
        let math = "we can infer \\(R_{\\mathrm{in}}\\) here";
        assert_eq!(clean_content("text", math), math);
    }

    #[test]
    fn junk_readings_are_dropped() {
        let page = page_with(
            vec![
                ocr_block("text", [0.0, 0.0, 10.0, 10.0], "1"),
                ocr_block("text", [0.0, 0.0, 10.0, 10.0], "—"),
                ocr_block("text", [0.0, 0.0, 10.0, 10.0], "[Unlabeled]"),
                ocr_block("text", [0.0, 0.0, 10.0, 10.0], "R3"),
            ],
            100.0,
            100.0,
        );
        let blocks = page_blocks(&page, [100.0, 100.0], 1).unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].content, "R3");
    }

    #[test]
    fn near_duplicates_merge_and_different_text_is_kept() {
        let page = page_with(
            vec![
                // Same line read twice with an overlapping box, the second copy truncated: one survivor,
                // unioned box (a re-detection never lands exactly on the first attempt)
                ocr_block("text", [0.0, 0.0, 100.0, 20.0], "The voltage divider delivers nearly all of it"),
                ocr_block("text", [0.0, 10.0, 100.0, 30.0], "The voltage divider delivers nearly all"),
                // Overlapping but different text: both stay
                ocr_block("text", [0.0, 100.0, 100.0, 120.0], "A different sentence entirely"),
                ocr_block("text", [0.0, 110.0, 100.0, 130.0], "Nothing in common with the other one"),
            ],
            100.0,
            200.0,
        );
        let blocks = page_blocks(&page, [100.0, 200.0], 1).unwrap();
        assert_eq!(blocks.len(), 3, "{:?}", blocks.iter().map(|b| &b.content).collect::<Vec<_>>());
        assert_eq!(blocks[0].content, "The voltage divider delivers nearly all of it");
        assert_eq!(blocks[0].loc[3], 30.0, "the survivor takes the union of both boxes");
        assert_eq!(blocks[1].content, "A different sentence entirely");
        assert_eq!(blocks[2].content, "Nothing in common with the other one");
    }

    #[test]
    fn image_blocks_never_absorb_text() {
        let page = page_with(
            vec![
                ocr_block("image", [0.0, 0.0, 100.0, 100.0], ""),
                ocr_block("text", [10.0, 10.0, 90.0, 90.0], "text inside the figure"),
            ],
            100.0,
            100.0,
        );
        let blocks = page_blocks(&page, [100.0, 100.0], 1).unwrap();
        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].content.is_empty());
        assert_eq!(blocks[1].content, "text inside the figure");
    }

    #[test]
    fn error_messages_cover_every_error_style() {
        assert_eq!(error_message(r#"{"detail":"forbidden"}"#), "forbidden");
        assert_eq!(
            error_message(r#"{"object":"error","error":{"message":"boom","type":"x"}}"#),
            "boom"
        );
        // Mistral's own shape is a top-level message, which has no `error` wrapper to look under.
        assert_eq!(
            error_message(r#"{"object":"error","message":"Invalid model","type":"invalid_request_error"}"#),
            "Invalid model"
        );
        assert_eq!(error_message("plain text"), "plain text");
    }

    #[test]
    fn token_helpers_match_the_python_rules() {
        assert_eq!(normalized("Ohm's Law, 3.5V"), "ohmslaw35v");
        assert_eq!(tokens("Ohm's Law, 3.5V"), vec!["ohm", "s", "law", "3", "5v"]);
        assert!(fuzzy_hit("connota", &["connotation".to_string()]));
        assert!(!fuzzy_hit("abc", &["xyz".to_string()]));
        assert!(texts_look_dual("(Sorry folks: that's as damping)", "(Sorry folks: that's as damping)"));
        assert!(texts_look_dual("The voltage divider delivers nearly all of it", "The voltage divider delivers nearly all"));
        assert!(!texts_look_dual("Ohm's law applies to resistors", "Capacitors store charge in a field"));
        assert_eq!(bigram_dice("abcd", "abcd"), 1.0);
    }

    #[test]
    fn request_body_follows_the_mistral_shape() {
        let client = OcrClient::new("http://127.0.0.1:9055", "t", None);
        assert_eq!(client.model, DEFAULT_MODEL);
        let payload = Payload::Whole("QUJD".to_string());
        let body = request_body(&client, &payload, &[1, 3]);
        assert_eq!(body["model"], serde_json::json!(DEFAULT_MODEL));
        assert_eq!(body["document"]["type"], serde_json::json!("document_url"));
        assert_eq!(
            body["document"]["document_url"],
            serde_json::json!("data:application/pdf;base64,QUJD")
        );
        // The client counts pages from 1, the protocol from 0.
        assert_eq!(body["pages"], serde_json::json!([0, 2]));
        assert_eq!(body["include_blocks"], serde_json::json!(true));
        // Off by default at both ends, but a provider is free to default it the other way, and the
        // reader never uses the crops.
        assert_eq!(body["include_image_base64"], serde_json::json!(false));
        // HTML is the table form our parser reads (markdown pipes cannot express rowspan).
        assert_eq!(body["table_format"], serde_json::json!("html"));

        // A slice is its own document, so the page filter would only re-number it.
        let payload = Payload::Slice("QUJD".to_string());
        let body = request_body(&client, &payload, &[1, 3]);
        assert!(body.get("pages").is_none());

        // A gateway/LiteLLM alias selects the upstream from the model field.
        let client = OcrClient::new("http://host", "t", Some("  deepseek-ocr  "));
        assert_eq!(client.model, "deepseek-ocr");
        let client = OcrClient::new("http://host", "t", Some("   "));
        assert_eq!(client.model, DEFAULT_MODEL);
    }

    #[test]
    fn retryable_statuses_split_transient_from_permanent() {
        assert!(matches!(retryable(429), Retry::Quota), "over quota: the caller parks globally");
        for status in [500, 502, 503, 504] {
            assert!(matches!(retryable(status), Retry::Transient), "{status}");
        }
        // A bad key or an unknown model answers the same way forever.
        for status in [400, 401, 403, 404, 422] {
            assert!(matches!(retryable(status), Retry::Never), "{status}");
        }
        assert!(RATE_LIMITED.ends_with(' '), "the marker is a prefix, never a whole message");
    }

    #[test]
    fn formula_readings_without_delimiters_are_wrapped() {
        // Bare LaTeX from a service that sends formulas undelimited: the cover would paint it as text.
        assert_eq!(clean_content("formula", "\\frac{a}{b} = c"), "$$\\frac{a}{b} = c$$");
        assert_eq!(clean_content("formula", "E = mc^2"), "$$E = mc^2$$");
        assert_eq!(clean_content("formula", "x_1 + x_2"), "$$x_1 + x_2$$");
        assert_eq!(clean_content("formula", "  \\sum_{i} a_i  "), "$$\\sum_{i} a_i$$");
        // Already delimited, in any of the four forms the renderer understands: left alone.
        for already in ["$$a+b$$", "\\[a+b\\]", "$a+b$", "\\(a+b\\)"] {
            assert_eq!(clean_content("formula", already), already);
        }
        // No LaTeX signal: a plain caption misclassified as a formula must stay readable prose.
        assert_eq!(clean_content("formula", "Figure 3.1"), "Figure 3.1");
        assert_eq!(clean_content("formula", "波长 lambda"), "波长 lambda");
        assert_eq!(clean_content("formula", ""), "");
        // Text blocks never get wrapped, even with plenty of LaTeX-looking characters in them.
        assert_eq!(clean_content("text", "E = mc^2 is the relation"), "E = mc^2 is the relation");
    }

    #[test]
    fn collect_updates_resolves_both_numberings() {
        let requested = |index: u32| MistralPageInput { index, size_pt: [100.0, 200.0] };
        let answer = |index: u32, text: &str| {
            let mut page = page_with(vec![ocr_block("text", [0.0, 0.0, 50.0, 20.0], text)], 100.0, 200.0);
            page.index = index;
            page
        };

        // A slice counts from 0 in the order asked, which is not the order we asked in: the reply is
        // mapped positionally against the ascending request (3 → 0, 5 → 1, 9 → 2).
        let (updates, refused) = collect_updates(
            &[requested(5), requested(3), requested(9)],
            vec![answer(0, "aa"), answer(1, "bb"), answer(2, "cc")],
        )
        .unwrap();
        assert!(refused.is_empty());
        let by_index: HashMap<u32, &str> = updates.iter().map(|(i, b)| (*i, b[0].content.as_str())).collect();
        assert_eq!(by_index[&3], "aa");
        assert_eq!(by_index[&5], "bb");
        assert_eq!(by_index[&9], "cc");

        // An unsliced request answers with absolute document indices, which the protocol counts from 0
        // where the bound JSON counts from 1.
        let (updates, _) =
            collect_updates(&[requested(20), requested(21)], vec![answer(19, "xx"), answer(20, "yy")]).unwrap();
        assert_eq!(updates.iter().map(|(i, _)| *i).collect::<Vec<u32>>(), vec![20, 21]);
        let by_index: HashMap<u32, &str> = updates.iter().map(|(i, b)| (*i, b[0].content.as_str())).collect();
        assert_eq!(by_index[&20], "xx");
        assert_eq!(by_index[&21], "yy");

        // The one case where the two numberings coincide: a request of exactly 1..n maps the same either
        // way, so the detection cannot pick wrong.
        let (updates, _) = collect_updates(&[requested(1), requested(2)], vec![answer(0, "aa"), answer(1, "bb")]).unwrap();
        let by_index: HashMap<u32, &str> = updates.iter().map(|(i, b)| (*i, b[0].content.as_str())).collect();
        assert_eq!(by_index[&1], "aa");
        assert_eq!(by_index[&2], "bb");

        // A whole batch defect is still an error: a numbering that never mentions our pages, a repeated
        // page, a batch that lists one twice.
        assert!(collect_updates(&[requested(4), requested(5)], vec![answer(7, "aa"), answer(8, "bb")])
            .unwrap_err()
            .contains("do not cover"));
        assert!(collect_updates(&[requested(4)], vec![answer(0, "aa"), answer(0, "bb")])
            .unwrap_err()
            .contains("twice"));
        assert!(collect_updates(&[requested(4), requested(4)], vec![answer(0, "aa"), answer(1, "bb")])
            .unwrap_err()
            .contains("repeats a page"));
        // A service that ignores the `pages` filter and answers the whole document is wasteful, not
        // wrong: our pages are there, so the batch proceeds.
        let (updates, _) = collect_updates(&[requested(4)], vec![answer(0, "aa"), answer(3, "dd"), answer(9, "zz")])
            .unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].1[0].content, "dd");
    }


    #[test]
    fn collect_updates_isolates_a_refused_page() {
        let requested = |index: u32| MistralPageInput { index, size_pt: [100.0, 200.0] };
        let good = {
            let mut page = page_with(vec![ocr_block("text", [0.0, 0.0, 50.0, 20.0], "kept")], 100.0, 200.0);
            page.index = 0;
            page
        };
        // Page 6 rendered at the wrong aspect (a rotation the service applied and we did not): it is
        // refused on its own, and pages 5 and 7 still come back with their blocks.
        let rotated = OcrPage {
            index: 1,
            dimensions: Some(Dimensions { width: 200.0, height: 100.0 }),
            blocks: Some(vec![ocr_block("text", [0.0, 0.0, 10.0, 10.0], "x")]),
        };
        let mut third = good.clone();
        third.index = 2;

        let (updates, refused) = collect_updates(&[requested(5), requested(6), requested(7)], vec![good, rotated, third]).unwrap();
        assert_eq!(updates.iter().map(|(i, _)| *i).collect::<Vec<u32>>(), vec![5, 7]);
        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0].index, 6);
        // The reason names the page as the user knows it, not as the slice numbered it.
        assert!(refused[0].reason.starts_with("page 6:"), "{}", refused[0].reason);
        assert!(refused[0].reason.contains("different page box"), "{}", refused[0].reason);
    }

    /// A PDF with `pages` pages of growing size, and optionally a `/Rotate` inherited from the page
    /// tree — the attribute a page-tree rewrite is most likely to drop silently.
    fn fixture_pdf(pages: u32, inherited_rotate: bool) -> Vec<u8> {
        let mut doc = lopdf::Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let mut kids = Vec::new();
        for n in 1..=pages {
            let content = doc.add_object(lopdf::Stream::new(lopdf::Dictionary::new(), Vec::new()));
            let mut page = lopdf::Dictionary::new();
            page.set("Type", lopdf::Object::Name(b"Page".to_vec()));
            page.set("Parent", lopdf::Object::Reference(pages_id));
            page.set(
                "MediaBox",
                lopdf::Object::Array(vec![
                    lopdf::Object::Integer(0),
                    lopdf::Object::Integer(0),
                    lopdf::Object::Integer(200 * n as i64),
                    lopdf::Object::Integer(400 * n as i64),
                ]),
            );
            page.set("Contents", lopdf::Object::Reference(content));
            kids.push(lopdf::Object::Reference(doc.add_object(page)));
        }
        let mut tree = lopdf::Dictionary::new();
        tree.set("Type", lopdf::Object::Name(b"Pages".to_vec()));
        tree.set("Kids", lopdf::Object::Array(kids));
        tree.set("Count", lopdf::Object::Integer(pages as i64));
        if inherited_rotate {
            tree.set("Rotate", lopdf::Object::Integer(90));
        }
        doc.objects.insert(pages_id, lopdf::Object::Dictionary(tree));
        let mut catalog = lopdf::Dictionary::new();
        catalog.set("Type", lopdf::Object::Name(b"Catalog".to_vec()));
        catalog.set("Pages", lopdf::Object::Reference(pages_id));
        let catalog_id = doc.add_object(catalog);
        doc.trailer.set("Root", lopdf::Object::Reference(catalog_id));
        let mut out = Vec::new();
        doc.save_to(&mut out).expect("the fixture must save");
        out
    }

    #[test]
    fn slice_extracts_only_the_wanted_pages() {
        let source = fixture_pdf(5, false);
        let source_doc = lopdf::Document::load_mem(&source).unwrap();
        let frame = |bytes: &[u8], page: u32| {
            let doc = lopdf::Document::load_mem(bytes).unwrap();
            page_frame(&doc, page).unwrap()
        };
        // Requested out of order: the slice is built in document order (that is what the answer's
        // 0..n-1 positions are mapped against), so its page 1 is the source's page 2.
        let encoded = slice(&source_doc, &[4, 2]).expect("a plain PDF must slice");
        let decoded = base64_decode(&encoded);
        let sliced = lopdf::Document::load_mem(&decoded).unwrap();
        assert_eq!(sliced.get_pages().len(), 2, "only the wanted pages travel");
        // Growing page sizes make a wrong slot visible.
        assert_eq!(frame(&decoded, 1), frame(&source, 2));
        assert_eq!(frame(&decoded, 2), frame(&source, 4));
        assert_eq!(frame(&decoded, 1).width_pt, 400.0);
        // The other pages left no trace: their objects are unreachable and pruned.
        assert!(
            sliced.objects.len() < source_doc.objects.len(),
            "the slice kept everything: {} of {} objects",
            sliced.objects.len(),
            source_doc.objects.len()
        );
        // The cached document is never mutated: it still holds all five pages.
        assert_eq!(source_doc.get_pages().len(), 5);

        // An inherited rotation must survive the flattening, or the service renders the page differently
        // from the viewer and every box on it lands wrong.
        let rotated_source = fixture_pdf(3, true);
        assert_eq!(frame(&rotated_source, 1).rotate, 90);
        let rotated_doc = lopdf::Document::load_mem(&rotated_source).unwrap();
        let encoded = slice(&rotated_doc, &[3]).expect("must slice");
        assert_eq!(frame(&base64_decode(&encoded), 1).rotate, 90);

        // A page the document does not have, and an encrypted file: fall back, never guess.
        assert!(slice(&source_doc, &[6]).unwrap_err().contains("does not exist"));
        assert!(slice(&source_doc, &[]).unwrap_err().contains("no pages"));
        let mut encrypted = lopdf::Document::load_mem(&source).unwrap();
        encrypted.trailer.set("Encrypt", lopdf::Object::Reference((9, 0)));
        let mut bytes = Vec::new();
        encrypted.save_to(&mut bytes).unwrap();
        let encrypted_doc = lopdf::Document::load_mem(&bytes).unwrap();
        assert!(slice(&encrypted_doc, &[1]).unwrap_err().contains("encrypted"));
    }

    fn base64_decode(encoded: &str) -> Vec<u8> {
        let value = |c: u8| -> Option<u8> { B64.iter().position(|b| *b == c).map(|p| p as u8) };
        let mut out = Vec::with_capacity(encoded.len() / 4 * 3);
        for chunk in encoded.as_bytes().chunks(4) {
            let numbers: Vec<Option<u8>> = chunk.iter().map(|c| value(*c)).collect();
            let b0 = numbers.first().copied().flatten().unwrap_or(0);
            let b1 = numbers.get(1).copied().flatten().unwrap_or(0);
            let triple = ((b0 as u32) << 18) | ((b1 as u32) << 12);
            out.push((triple >> 16) as u8);
            if chunk.len() > 2 && chunk[2] != b'=' {
                let b2 = numbers[2].unwrap_or(0);
                out.push((((b1 as u32) << 12 | (b2 as u32) << 6) >> 8) as u8);
                if chunk.len() > 3 && chunk[3] != b'=' {
                    let b3 = numbers[3].unwrap_or(0);
                    out.push((((b2 as u32) << 6) | b3 as u32) as u8);
                }
            }
        }
        out
    }

    /// A real answer, captured from `POST /v1/ocr` on the local service (page 20 of a 622-page book,
    /// every block kind our pipeline emits except a table). Pins the wire shape and the conversion.
    #[test]
    fn real_service_payload_converts_to_blocks() {
        let raw = include_str!("../tests/fixtures/mistral_page.json");
        let parsed: OcrResponse = serde_json::from_str(raw).expect("the captured payload must parse");
        assert_eq!(parsed.pages.len(), 1);
        let page = &parsed.pages[0];
        assert_eq!(page.index, 19, "the service numbers pages from 0");

        // The viewer's page size for that page (940x1183 px rendered at 144 dpi = 470x591.5 pt)
        let size_pt = [470.0, 591.5];
        let blocks = page_blocks(page, size_pt, 20).expect("the captured page must convert");
        assert!(blocks.len() >= 15, "unexpectedly few blocks: {}", blocks.len());

        for block in &blocks {
            assert!(is_client_type(&block.kind), "foreign type leaked: {}", block.kind);
            assert!(block.translation.is_none());
            assert!(
                block.loc[0] >= 0.0
                    && block.loc[1] >= 0.0
                    && block.loc[2] <= size_pt[0]
                    && block.loc[3] <= size_pt[1]
                    && block.loc[2] > block.loc[0]
                    && block.loc[3] > block.loc[1],
                "box outside the page: {:?}",
                block.loc
            );
            if block.kind != "image" {
                assert!(!block.content.trim().is_empty(), "empty {:?} block", block.kind);
            }
        }

        let kinds: Vec<&str> = blocks.iter().map(|b| b.kind.as_str()).collect();
        for expected in ["formula", "image", "header", "figure_title", "text"] {
            assert!(kinds.contains(&expected), "no {expected} block in {kinds:?}");
        }
        // Our own labels survive the round trip (the service sends type + label)
        let image = blocks.iter().find(|b| b.kind == "image").unwrap();
        assert!(image.content.is_empty(), "figure content is the client's empty contract");
        // Captions arrive wrapped in <center>, which must not reach the cover
        for caption in blocks.iter().filter(|b| b.kind == "figure_title") {
            assert!(!caption.content.contains("<center>"), "{}", caption.content);
        }
    }
}
