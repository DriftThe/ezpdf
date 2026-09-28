//! PaddleOCR-VL table markup → grid. Marks: `<fcel>` new cell (text follows), `<ecel>` empty,
//! `<lcel>` merge left (occupies a slot, left colspan += 1), `<ucel>`/`<xcel>` merge up (rendered
//! blank), `<nl>` next row.
//!
//! cols = max slot count per row. Short rows (page-boundary truncation) get a blank spanning cell;
//! unshaped markup → None (no translate, no cover, original pixels).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Table block type name (Block.kind), shared so the literal can't drift.
pub const TABLE: &str = "table";

/// Grid caps: exceeding any is a parse failure (malformed markup shouldn't break LLM or render).
const MAX_ROWS: usize = 200;
const MAX_SLOTS: usize = 64;
const MAX_CELLS: usize = 4000;

/// Cell: text + horizontal colspan (from `<lcel>`). Vertical merges are not expressed in v1.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TableCell {
    pub text: String,
    pub colspan: u32,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TableRow {
    pub cells: Vec<TableCell>,
}

/// Table grid: `cols` is the slot count (a row's cell colspans sum to cols).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TableGrid {
    pub cols: u32,
    pub rows: Vec<TableRow>,
}

#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Fcel,
    Ecel,
    Lcel,
    Ucel,
    Xcel,
}

const MARKS: [(&str, Mark); 5] = [
    ("<fcel>", Mark::Fcel),
    ("<ecel>", Mark::Ecel),
    ("<lcel>", Mark::Lcel),
    ("<ucel>", Mark::Ucel),
    ("<xcel>", Mark::Xcel),
];

fn mark_at(s: &str) -> Option<(Mark, usize)> {
    MARKS.iter().find(|(pat, _)| s.starts_with(pat)).map(|(pat, kind)| (*kind, pat.len()))
}

/// One markup line → (real cells, slot count). No marks → None (the row is ignored).
fn parse_line(line: &str) -> Option<(Vec<TableCell>, usize)> {
    let mut positions: Vec<(Mark, usize, usize)> = Vec::new();
    let mut cursor = 0;
    while cursor < line.len() {
        let rest = &line[cursor..];
        match mark_at(rest) {
            Some((kind, len)) => {
                positions.push((kind, cursor, cursor + len));
                cursor += len;
            }
            None => cursor += rest.chars().next().map(|c| c.len_utf8()).unwrap_or(1),
        }
    }
    if positions.is_empty() {
        return None;
    }
    let mut cells: Vec<TableCell> = Vec::new();
    let mut slots = 0usize;
    for (i, (kind, _start, end)) in positions.iter().enumerate() {
        let next = positions.get(i + 1).map(|p| p.1).unwrap_or(line.len());
        let text = line[*end..next].trim().to_string();
        slots += 1;
        if *kind == Mark::Lcel {
            match cells.last_mut() {
                Some(last) => last.colspan += 1,
                None => cells.push(TableCell { text: String::new(), colspan: 1 }),
            }
        } else {
            cells.push(TableCell { text, colspan: 1 });
        }
    }
    Some((cells, slots))
}

pub fn parse_markup(content: &str) -> Option<TableGrid> {
    let mut rows: Vec<TableRow> = Vec::new();
    let mut slots: Vec<usize> = Vec::new();
    for line in content.split("<nl>") {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((cells, n)) = parse_line(line) {
            rows.push(TableRow { cells });
            slots.push(n);
        }
    }
    if rows.is_empty() {
        return None;
    }
    let cols = slots.iter().copied().max().unwrap_or(0);
    let total: usize = rows.iter().map(|r| r.cells.len()).sum();
    if cols == 0 || cols > MAX_SLOTS || rows.len() > MAX_ROWS || total > MAX_CELLS {
        return None;
    }
    for (row, n) in rows.iter_mut().zip(slots.iter()) {
        if *n < cols {
            row.cells.push(TableCell { text: String::new(), colspan: (cols - n) as u32 });
        }
    }
    Some(TableGrid { cols: cols as u32, rows })
}

/// HTML table → grid. Other Mistral-shaped OCR services (DeepSeek-OCR behind such a gateway, for one)
/// answer tables as HTML, our own pipeline answers with the markup above; both feed the same grid.
///
/// `colspan` maps straight onto `TableCell::colspan`. `rowspan` has no place in the grid model, so the
/// slots it spans in the rows below are emitted blank — the convention `<xcel>` already uses for a
/// vertically merged cell — which keeps the grid rectangular for the LLM round trip.
pub fn parse_html(content: &str) -> Option<TableGrid> {
    let lower = content.to_ascii_lowercase();
    let start = lower.find("<table")?;
    let body = &content[start..];

    let mut rows: Vec<TableRow> = Vec::new();
    let mut slots: Vec<usize> = Vec::new();
    let mut spanned: Vec<u32> = Vec::new(); // slots a rowspan still covers (decremented once per row)
    let mut row = RowBuilder::default();
    let mut cell_text: Option<String> = None;
    let mut cell_colspan = 1u32;
    let mut cell_rowspan = 1u32;
    let mut in_row = false;

    // A tiny scanner is enough: we only care about table structure, everything else is cell text.
    let mut rest = body;
    while let Some(open) = rest.find('<') {
        if let Some(buffer) = cell_text.as_mut() {
            buffer.push_str(&rest[..open]);
        }
        rest = &rest[open..];
        let Some(close) = rest.find('>') else { break };
        let tag = &rest[1..close];
        rest = &rest[close + 1..];

        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        let closing = tag.starts_with('/');
        match name.as_str() {
            "table" if closing => break, // only the first table in the block
            "tr" | "td" | "th" => {
                // A cell ends at its closing tag or at the next cell/row start (implicit close, common
                // in HTML produced by a model): flush whatever is pending before deciding what this is.
                if let Some(buffer) = cell_text.take() {
                    row.push(&mut spanned, buffer, cell_colspan, cell_rowspan);
                }
                if name == "tr" {
                    if in_row {
                        let (built, count) = row.finish(&mut spanned);
                        rows.push(built);
                        slots.push(count);
                        if rows.len() > MAX_ROWS {
                            return None;
                        }
                    }
                    in_row = !closing;
                } else if !closing {
                    cell_colspan = attribute(tag, "colspan").unwrap_or(1).max(1);
                    cell_rowspan = attribute(tag, "rowspan").unwrap_or(1).max(1);
                    cell_text = Some(String::new());
                }
            }
            "br" => {
                if let Some(buffer) = cell_text.as_mut() {
                    buffer.push('\n');
                }
            }
            _ => {}
        }
        if rows.len() > MAX_ROWS {
            return None;
        }
    }
    if let Some(buffer) = cell_text.take() {
        row.push(&mut spanned, buffer, cell_colspan, cell_rowspan);
    }
    if in_row {
        let (built, count) = row.finish(&mut spanned);
        rows.push(built);
        slots.push(count);
    }
    if rows.is_empty() || rows.len() > MAX_ROWS {
        return None;
    }

    let cols = slots.iter().copied().max().unwrap_or(0);
    let total: usize = rows.iter().map(|r| r.cells.len()).sum();
    if cols == 0 || cols > MAX_SLOTS || total > MAX_CELLS {
        return None;
    }
    for (row, n) in rows.iter_mut().zip(slots.iter()) {
        if *n < cols {
            row.cells.push(TableCell { text: String::new(), colspan: (cols - n) as u32 });
        }
    }
    Some(TableGrid { cols: cols as u32, rows })
}

/// Grid from either dialect: our pipeline's `<fcel>` markup first, HTML tables second.
pub fn parse_any(content: &str) -> Option<TableGrid> {
    parse_markup(content).or_else(|| parse_html(content))
}

/// One `<tr>` under construction: cells plus the slot the next cell lands on.
#[derive(Default)]
struct RowBuilder {
    cells: Vec<TableCell>,
    slot: usize,
}

impl RowBuilder {
    /// Cells are placed left to right; every slot an earlier rowspan still covers is emitted blank.
    fn take_spanned(&mut self, spanned: &mut [u32]) {
        while let Some(counter) = spanned.get_mut(self.slot) {
            if *counter == 0 {
                break;
            }
            *counter -= 1;
            self.cells.push(TableCell { text: String::new(), colspan: 1 });
            self.slot += 1;
        }
    }

    fn push(&mut self, spanned: &mut Vec<u32>, text: String, colspan: u32, rowspan: u32) {
        self.take_spanned(spanned);
        let colspan = colspan.min(MAX_SLOTS as u32);
        self.slot += colspan as usize;
        if rowspan > 1 {
            spanned.resize(self.slot.max(spanned.len()), 0);
            let start = self.slot - colspan as usize;
            for counter in spanned[start..self.slot].iter_mut() {
                *counter = (rowspan - 1).min(MAX_ROWS as u32);
            }
        }
        self.cells.push(TableCell { text: clean_cell_text(&text), colspan });
    }

    fn finish(&mut self, spanned: &mut [u32]) -> (TableRow, usize) {
        self.take_spanned(spanned); // trailing slots a rowspan covers still belong to this row
        let count = self.slot;
        self.slot = 0;
        (TableRow { cells: std::mem::take(&mut self.cells) }, count)
    }
}

/// One numeric attribute of a tag (`colspan="2"`, `rowspan='3'`, unquoted) — `None` when absent/invalid.
fn attribute(tag: &str, name: &str) -> Option<u32> {
    let lower = tag.to_ascii_lowercase();
    let at = lower.find(name)?;
    let rest = tag[at + name.len()..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let digits: String = rest
        .trim_start_matches(['"', '\''])
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// Cell text: nested tags dropped, entities decoded, whitespace collapsed.
fn clean_cell_text(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        match rest[open..].find('>') {
            Some(close) => rest = &rest[open + close + 1..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Real cell count per row (for shape validation; includes padded/merged cells).
pub fn row_lengths(grid: &TableGrid) -> Vec<usize> {    grid.rows.iter().map(|r| r.cells.len()).collect()
}

/// Translation payload: `{"table": [[cell text, ...], ...]}` (real cells only, in grid order).
pub fn payload(grid: &TableGrid) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = grid
        .rows
        .iter()
        .map(|r| {
            serde_json::Value::Array(
                r.cells.iter().map(|c| serde_json::Value::String(c.text.clone())).collect(),
            )
        })
        .collect();
    serde_json::json!({ "table": rows })
}

/// Source matrix (the disabled-translation persisted value): 2-D string JSON, same shape as `payload`.
pub fn source_matrix(grid: &TableGrid) -> String {
    payload(grid).to_string()
}

/// Validate a table result against the request shape (row + per-row cell counts); returns compact JSON, numbers/bools normalized.
pub fn validate_value(value: &serde_json::Value, expected: &[usize]) -> Result<String, String> {
    let rows = match value {
        serde_json::Value::Array(rows) => rows,
        serde_json::Value::Object(map) => match map.get("table").and_then(|v| v.as_array()) {
            Some(rows) => rows,
            None => return Err("table result has no `table` array".into()),
        },
        _ => return Err("table result is neither a 2-D array nor {\"table\": [...]}".into()),
    };
    if rows.len() != expected.len() {
        return Err(format!("table row count mismatch: expected {} got {}", expected.len(), rows.len()));
    }
    let mut out: Vec<serde_json::Value> = Vec::with_capacity(rows.len());
    for (i, row) in rows.iter().enumerate() {
        let serde_json::Value::Array(cells) = row else {
            return Err(format!("table row {} is not an array", i));
        };
        if cells.len() != expected[i] {
            return Err(format!(
                "table row {} length mismatch: expected {} got {}",
                i,
                expected[i],
                cells.len()
            ));
        }
        let mut normalized: Vec<serde_json::Value> = Vec::with_capacity(cells.len());
        for cell in cells {
            normalized.push(match cell {
                serde_json::Value::Null => serde_json::Value::Null,
                serde_json::Value::String(s) => serde_json::Value::String(s.clone()),
                serde_json::Value::Number(n) => serde_json::Value::String(n.to_string()),
                serde_json::Value::Bool(b) => serde_json::Value::String(b.to_string()),
                other => {
                    return Err(format!("table row {i} has a non-scalar cell: {other}"));
                }
            });
        }
        out.push(serde_json::Value::Array(normalized));
    }
    Ok(serde_json::Value::Array(out).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(grid: &TableGrid) -> Vec<Vec<String>> {
        grid.rows
            .iter()
            .map(|r| r.cells.iter().map(|c| c.text.clone()).collect())
            .collect()
    }

    fn spans(grid: &TableGrid) -> Vec<Vec<u32>> {
        grid.rows.iter().map(|r| r.cells.iter().map(|c| c.colspan).collect()).collect()
    }

    #[test]
    fn parses_a_simple_table() {
        // Real data p23: 3 columns, 4 rows, with inline LaTeX
        let content = "<fcel>参 数<fcel>冬 季<fcel>夏 季<nl>\
                       <fcel>温度(℃)<fcel>18～24<fcel>25～28<nl>\
                       <fcel>风速(m/s)<fcel>\\(\\leq0.2\\)<fcel>\\(\\leq0.3\\)<nl>\
                       <fcel>相对湿度(%)<fcel>—<fcel>40～70<nl>";
        let g = parse_markup(content).expect("parses");
        assert_eq!(g.cols, 3);
        assert_eq!(row_lengths(&g), vec![3, 3, 3, 3]);
        assert_eq!(spans(&g), vec![vec![1, 1, 1]; 4]);
        assert_eq!(
            texts(&g)[2],
            vec!["风速(m/s)".to_string(), "\\(\\leq0.2\\)".to_string(), "\\(\\leq0.3\\)".to_string()]
        );
    }

    #[test]
    fn merges_left_cells_into_colspan() {
        // Real data p137 header: one cell spans 5 columns → 6 slots but only 2 real cells
        let content = "<fcel>(2)<fcel>河北(10)<lcel><lcel><lcel><lcel><nl>\
                       <fcel>塘沽<fcel>石家庄<fcel>唐山<fcel>邢台<fcel>保定<fcel>张家口<nl>";
        let g = parse_markup(content).expect("parses");
        assert_eq!(g.cols, 6);
        assert_eq!(row_lengths(&g), vec![2, 6]);
        assert_eq!(spans(&g)[0], vec![1, 5]);
        assert_eq!(texts(&g)[0], vec!["(2)".to_string(), "河北(10)".to_string()]);
    }

    #[test]
    fn pads_truncated_tail_row() {
        // Real data p137 last row: truncated at a page boundary with 3 slots → one padded cell
        let mut content = String::new();
        for _ in 0..3 {
            content.push_str("<fcel>26.9<fcel>26.8<fcel>26.3<fcel>26.9<fcel>26.6<fcel>22.6<nl>");
        }
        content.push_str("<fcel>28.8<fcel>30.8<fcel><nl>");
        let g = parse_markup(&content).expect("parses");
        assert_eq!(g.cols, 6);
        assert_eq!(row_lengths(&g), vec![6, 6, 6, 4]);
        assert_eq!(spans(&g)[3], vec![1, 1, 1, 3]);
        assert_eq!(texts(&g)[3], vec!["28.8".to_string(), "30.8".to_string(), String::new(), String::new()]);
    }

    #[test]
    fn vertical_merge_becomes_blank_cell() {
        // Real data p140: <ucel> merges upward → v1 renders a blank cell (still occupies a slot)
        let content = "<fcel>台站信息<fcel>北纬<fcel>36°45'<nl><ucel><fcel>东经<fcel>119°11'<nl>";
        let g = parse_markup(content).expect("parses");
        assert_eq!(g.cols, 3);
        assert_eq!(row_lengths(&g), vec![3, 3]);
        assert_eq!(texts(&g)[1], vec![String::new(), "东经".to_string(), "119°11'".to_string()]);
    }

    #[test]
    fn rejects_unusable_markup() {
        assert!(parse_markup("").is_none());
        assert!(parse_markup("   <nl><nl>  ").is_none());
        assert!(parse_markup("普通文本，没有表格标记").is_none());
        let too_many = "<fcel>a<nl>".repeat(MAX_ROWS + 1);
        assert!(parse_markup(&too_many).is_none());
        let too_wide = format!("{}<nl>", "<fcel>x".repeat(MAX_SLOTS + 1));
        assert!(parse_markup(&too_wide).is_none());
    }

    #[test]
    fn validate_accepts_matching_shapes_and_normalizes() {
        let expected = vec![2usize, 1];
        let ok = serde_json::json!([["译文", null], [42]]);
        assert_eq!(validate_value(&ok, &expected).unwrap(), "[[\"译文\",null],[\"42\"]]");
        let wrapped = serde_json::json!({"table": [["a", "b"], ["c"]]});
        assert_eq!(validate_value(&wrapped, &expected).unwrap(), "[[\"a\",\"b\"],[\"c\"]]");
    }

    #[test]
    fn validate_rejects_broken_shapes() {
        let expected = vec![2usize, 1];
        assert!(validate_value(&serde_json::json!([["a", "b"]]), &expected).is_err());
        assert!(validate_value(&serde_json::json!([["a"], ["b"]]), &expected).is_err());
        assert!(validate_value(&serde_json::json!("a|b|c"), &expected).is_err());
        assert!(validate_value(&serde_json::json!([[["a"], "b"], ["c"]]), &expected).is_err());
    }

    #[test]
    fn source_matrix_matches_payload_shape() {
        let content = "<fcel>a<fcel>b<lcel><nl><fcel>c<nl>";
        let g = parse_markup(content).expect("parses");
        assert_eq!(g.cols, 3, "b 跨 2 列 → 3 个列位");
        assert_eq!(spans(&g)[0], vec![1, 2]);
        assert_eq!(source_matrix(&g), "{\"table\":[[\"a\",\"b\"],[\"c\",\"\"]]}");
        assert_eq!(row_lengths(&g), vec![2, 2]);
    }
}
