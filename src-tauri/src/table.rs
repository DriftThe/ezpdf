//! 表格块解析（用户 2026-09-16）：PaddleOCR-VL 的表格标记流 → 网格结构。
//!
//! 标记词汇（PP-StructureV3 / PaddleOCR-VL 表格文本流，实测本书 109 张表全部命中）：
//! - `<fcel>` 新单元格（后跟单元格文本，可能是空）
//! - `<ecel>` 空单元格
//! - `<lcel>` 与左侧单元格合并（占一个列位，不产生新格 → 左格 colspan += 1）
//! - `<ucel>` 与上方单元格合并（v1 渲染成空格子：统计类表格视觉等价且实现简单）
//! - `<xcel>` 交叉合并（同上）
//! - `<nl>` 换行：下一个表格行
//!
//! 列数 = 各行"列位"数的最大值。列位 = 新格标记数 + `<lcel>` 数；实测 52/109 张表有且
//! 只有**末行**列位偏少（页边界把表格切断），所以短行一律右侧补一个空跨列格，不当失败。
//! 解析不出形状（无单元格标记、行列数离谱）→ None：该块不送翻、不覆盖，原 PDF 像素直出。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// 网格上限：超出一律当解析失败（畸形标记不该把 LLM 请求或渲染拖垮）
const MAX_ROWS: usize = 200;
const MAX_SLOTS: usize = 64;
const MAX_CELLS: usize = 4000;

/// 单元格：文本 + 横向跨列数（`<lcel>` 合并的结果）。纵向合并 v1 不表达。
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TableCell {
    pub text: String,
    /// 跨列数（≥1）；`<lcel>` 每出现一次 +1
    pub colspan: u32,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TableRow {
    pub cells: Vec<TableCell>,
}

/// 表格网格：`cols` 是列位数（各行真实单元格的 colspan 之和 = cols）
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

/// 一行标记流 → (真实单元格, 列位数)。无标记 → None（该行整个忽略）
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

/// 解析表格标记流 → 网格；形状不可信 → None
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
    // 短行（实测只有末行，页边界截断）右侧补一个空跨列格，补齐到 cols
    for (row, n) in rows.iter_mut().zip(slots.iter()) {
        if *n < cols {
            row.cells.push(TableCell { text: String::new(), colspan: (cols - n) as u32 });
        }
    }
    Some(TableGrid { cols: cols as u32, rows })
}

/// 每行真实单元格数（结果形状校验用；含 `<lcel>` 合并后再补齐的格子）
pub fn row_lengths(grid: &TableGrid) -> Vec<usize> {
    grid.rows.iter().map(|r| r.cells.len()).collect()
}

/// 送翻负载：`{"table": [[单元格文本, ...], ...]}`（只含真实单元格，行内顺序即网格顺序）
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

/// 原文矩阵（翻译禁用路径的落盘值）：二维字符串 JSON，与 `payload` 同形
pub fn source_matrix(grid: &TableGrid) -> String {
    payload(grid).to_string()
}

/// 表块结果校验：形状必须与请求一致（行数 + 每行单元格数），元素是字符串或 null。
/// 通过 → 归一化后的紧凑 JSON 文本（数字/布尔统一成字符串，便于前端只处理 string|null）
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
        // 真实数据 p23：3 列 4 行，含行内 LaTeX
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
        // 真实数据 p137 表头：<(2)|河北(10) 跨 5 列>，6 个列位但只有 2 个真实单元格
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
        // 真实数据 p137 末行：页边界把行截断，只有 3 个列位 → 右侧补一个空跨列格
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
        // 真实数据 p140：<ucel> 与上方合并 → v1 渲染成空格子（仍占一个列位）
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
        // 行数超限
        let too_many = "<fcel>a<nl>".repeat(MAX_ROWS + 1);
        assert!(parse_markup(&too_many).is_none());
        // 列位超限
        let too_wide = format!("{}<nl>", "<fcel>x".repeat(MAX_SLOTS + 1));
        assert!(parse_markup(&too_wide).is_none());
    }

    #[test]
    fn validate_accepts_matching_shapes_and_normalizes() {
        let expected = vec![2usize, 1];
        let ok = serde_json::json!([["译文", null], [42]]);
        assert_eq!(validate_value(&ok, &expected).unwrap(), "[[\"译文\",null],[\"42\"]]");
        // {"table": [...]} 包装也接受
        let wrapped = serde_json::json!({"table": [["a", "b"], ["c"]]});
        assert_eq!(validate_value(&wrapped, &expected).unwrap(), "[[\"a\",\"b\"],[\"c\"]]");
    }

    #[test]
    fn validate_rejects_broken_shapes() {
        let expected = vec![2usize, 1];
        // 行数不符
        assert!(validate_value(&serde_json::json!([["a", "b"]]), &expected).is_err());
        // 行内长度不符
        assert!(validate_value(&serde_json::json!([["a"], ["b"]]), &expected).is_err());
        // 非数组
        assert!(validate_value(&serde_json::json!("a|b|c"), &expected).is_err());
        // 嵌套数组当单元格
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
