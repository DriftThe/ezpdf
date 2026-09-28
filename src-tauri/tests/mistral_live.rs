//! Live checks for the Mistral-shaped OCR route (`parse_pdf_mistral`), opt-in because they need a real
//! PDF (and, for the round trip, a running service):
//!
//! ```text
//! EZPDF_LIVE_OCR=http://127.0.0.1:9067 EZPDF_LIVE_TOKEN=<token> EZPDF_LIVE_PDF=<path to a multi-page PDF> \
//!   cargo test --test mistral_live -- --ignored --nocapture
//! ```
//!
//! Both pin what a unit test cannot: the PDF is read from the repo by id and sliced down to the wanted
//! pages before being sent as one base64 data URI (falling back to the whole document when it cannot be
//! sliced), the answer's page numbers map back onto the bound JSON's 1-based ones, and the blocks that
//! land on disk carry `loc` inside the page the frontend declared.

use std::path::{Path, PathBuf};

/// 1-based pages the test asks for; the PDF must have at least this many pages. Two pages, requested out
/// of order, so the slice-local numbering (0..n-1 in ascending request order) is exercised too.
const PAGES: [u32; 2] = [20, 18];

fn env_or_skip(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// A throwaway repo holding `pdf` plus the bound JSON skeleton the writer needs. `label` keeps the two
/// tests in this file off each other's directory (cargo runs them in parallel threads of one process).
fn repo_with_pdf(label: &str, pdf: &Path) -> (String, String) {
    let root = std::env::temp_dir().join(format!("ezpdf-mistral-live-{}-{label}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let name = pdf.file_stem().unwrap().to_string_lossy().to_string();
    std::fs::copy(pdf, root.join(format!("{name}-live.pdf"))).unwrap();
    std::fs::write(
        root.join(".ezrepo"),
        format!(r#"{{"folders":[],"pdfs":[{{"id":"live","name":"{name}","bind":"{name}-live.json","belong":null}}]}}"#),
    )
    .unwrap();
    let total = *PAGES.iter().max().unwrap();
    let pages: Vec<String> = (1..=total)
        .map(|i| format!(r#"{{"index":{i},"finished":false,"translated":false,"blocks":[]}}"#))
        .collect();
    std::fs::write(
        root.join(format!("{name}-live.json")),
        format!(r#"{{"status":"Pending","pages":[{}]}}"#, pages.join(",")),
    )
    .unwrap();
    let root_str = std::fs::canonicalize(&root).unwrap().to_string_lossy().to_string();
    (root_str, name)
}

/// The page's size in pt the way pdfjs sees it, so the aspect guard is exercised against the truth
/// rather than a guess. Uses the same reader the slicer verifies with (MediaBox/CropBox and rotation
/// inherited from the page tree).
fn page_size_pt(path: &Path, page: u32) -> Option<[f64; 2]> {
    let doc = lopdf::Document::load(path).ok()?;
    let frame = ezpdf_lib::mistral::page_frame(&doc, page)?;
    Some([frame.width_pt, frame.height_pt])
}

/// Which payload a real book produces. A book that silently falls back to uploading itself in full is
/// the one thing the slicing is there to prevent, so this fails loudly rather than printing.
#[test]
#[ignore = "needs a real PDF (EZPDF_LIVE_PDF); no service required"]
fn live_slice_decision() {
    let Some(pdf) = env_or_skip("EZPDF_LIVE_PDF") else {
        eprintln!("set EZPDF_LIVE_PDF to run this test");
        return;
    };
    let (root, _name) = repo_with_pdf("slice", Path::new(&pdf));
    // Twice: the first call parses the book (its cost belongs to the first batch), the second is what
    // every later batch pays.
    let first = std::time::Instant::now();
    let payload = ezpdf_lib::mistral::payload_for(&root, "live", &PAGES).expect("the payload must be built");
    let cold = first.elapsed();
    let second = std::time::Instant::now();
    let _ = ezpdf_lib::mistral::payload_for(&root, "live", &PAGES).expect("the payload must be built");
    let warm = second.elapsed();
    match &payload {
        ezpdf_lib::mistral::Payload::Slice(encoded) => {
            // The whole point: two pages out of a book must not cost the book's own upload. A real
            // scanned page is one image stream, so even a generous bound is far below the file.
            let whole = std::fs::metadata(&pdf).unwrap().len() as usize;
            println!(
                "sliced: {} base64 bytes for pages {PAGES:?} (file {whole} bytes); first {cold:?}, next {warm:?}",
                encoded.len()
            );
            assert!(
                encoded.len() < whole / 2,
                "the slice is {} base64 bytes for a {whole}-byte file",
                encoded.len()
            );
        }
        ezpdf_lib::mistral::Payload::Whole(encoded) => {
            panic!("fell back to the whole document ({} base64 bytes)", encoded.len());
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
#[ignore = "needs a running Mistral-shaped OCR service and a real PDF (see the module docs)"]
fn live_mistral_round_trip() {
    let (Some(base), Some(token), Some(pdf)) = (
        env_or_skip("EZPDF_LIVE_OCR"),
        env_or_skip("EZPDF_LIVE_TOKEN"),
        env_or_skip("EZPDF_LIVE_PDF"),
    ) else {
        eprintln!("set EZPDF_LIVE_OCR / EZPDF_LIVE_TOKEN / EZPDF_LIVE_PDF to run this test");
        return;
    };

    let source = PathBuf::from(&pdf);
    let (root_str, name) = repo_with_pdf("roundtrip", &source);
    let root = PathBuf::from(&root_str);
    match ezpdf_lib::mistral::payload_for(&root_str, "live", &PAGES).expect("the payload must be built") {
        ezpdf_lib::mistral::Payload::Slice(encoded) => println!("sending a slice ({} base64 bytes)", encoded.len()),
        ezpdf_lib::mistral::Payload::Whole(encoded) => {
            println!("WARNING: sending the whole document ({} base64 bytes)", encoded.len())
        }
    }

    let sizes: Vec<[f64; 2]> = PAGES
        .iter()
        .map(|page| page_size_pt(&source, *page).expect("the PDF's page size must be readable"))
        .collect();
    let inputs: Vec<ezpdf_lib::mistral::MistralPageInput> = PAGES
        .iter()
        .zip(&sizes)
        .map(|(page, size_pt)| ezpdf_lib::mistral::MistralPageInput {
            index: *page,
            size_pt: *size_pt,
        })
        .collect();
    let outcome = tauri::async_runtime::block_on(ezpdf_lib::parse::parse_batch_mistral(
        &root_str,
        "live",
        inputs,
        &base,
        &token,
        None,
    ))
    .expect("the live OCR round trip failed");

    // A page the client cannot place must be reported, never silently dropped.
    assert!(
        outcome.refused_pages.is_empty(),
        "pages refused by the live round trip: {:?}",
        outcome.refused_pages
    );
    assert_eq!(outcome.updated_pages.len(), PAGES.len());

    for (page_no, size_pt) in PAGES.iter().zip(&sizes) {
        let page = outcome
            .updated_pages
            .iter()
            .find(|p| p.index == *page_no)
            .unwrap_or_else(|| panic!("page {page_no} must come back"));
        assert!(page.finished && !page.translated, "a fresh OCR page is finished but not translated");
        assert!(!page.blocks.is_empty(), "no blocks came back for page {page_no}");

        for block in &page.blocks {
            assert!(block.translation.is_none());
            assert!(
                block.loc[0] >= 0.0
                    && block.loc[1] >= 0.0
                    && block.loc[2] <= size_pt[0] + 0.01
                    && block.loc[3] <= size_pt[1] + 0.01
                    && block.loc[2] > block.loc[0]
                    && block.loc[3] > block.loc[1],
                "box outside the declared page: {:?} ({})",
                block.loc,
                block.kind
            );
        }
    }

    // The same blocks must be on disk (Rust is the only writer of the bound JSON)
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join(format!("{name}-live.json"))).unwrap())
            .unwrap();
    for page_no in PAGES {
        let landed = outcome.updated_pages.iter().find(|p| p.index == page_no).unwrap();
        let stored = written["pages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["index"].as_u64() == Some(page_no as u64))
            .unwrap();
        assert_eq!(
            stored["blocks"].as_array().unwrap().len(),
            landed.blocks.len(),
            "the writer stored a different number of blocks for page {page_no}"
        );
    }
    assert_eq!(written["status"], serde_json::json!("Processing"));

    for page in &outcome.updated_pages {
        let kinds: std::collections::BTreeSet<&str> =
            page.blocks.iter().map(|b| b.kind.as_str()).collect();
        println!("page {}: {} blocks, kinds {kinds:?}", page.index, page.blocks.len());
    }
    let _ = std::fs::remove_dir_all(&root);
}
