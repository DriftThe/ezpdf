/**
 * 表格渲染侧的取用（用户 2026-09-16）：
 *
 * 标记流（`<fcel>` / `<nl>` …）由 **Rust 侧解析**（src-tauri/src/table.rs）并落进绑定 JSON 的
 * `block.grid`，译文则存成二维矩阵的 JSON 文本（`block.translation`）。前端只解释这两者，
 * 不再解析标记 —— 单一解析器，避免两侧列映射不一致造成静默错位。
 *
 * - `grid.rows[i].cells[j]` 是**真实单元格**（`<lcel>` 已折进 `colspan`），顺序即网格顺序；
 * - 译文矩阵与真实单元格一一对应；元素为 null（该格与目标语言相同）时回落原文。
 */

import type { TableGrid } from "../types/domain";

/** 译文矩阵：行 × 真实单元格；null = 该格内容与目标语言相同（渲染回落原文） */
export type TableMatrix = (string | null)[][];

/** 解析 `block.translation` 的矩阵 JSON；不是合法矩阵（或形状对不上网格）→ null */
export function parseTableMatrix(translation: string | null, grid: TableGrid): TableMatrix | null {
  if (!translation) return null;
  let raw: unknown;
  try {
    raw = JSON.parse(translation);
  } catch {
    return null;
  }
  // 容错：允许 {"table": [[...]]} 包装（提示词要求裸数组，模型偶尔会加壳）
  const rows = Array.isArray(raw)
    ? raw
    : raw && typeof raw === "object" && Array.isArray((raw as { table?: unknown }).table)
      ? ((raw as { table: unknown[] }).table)
      : null;
  if (!rows || rows.length !== grid.rows.length) return null;
  const matrix: TableMatrix = [];
  for (let i = 0; i < rows.length; i++) {
    const row = rows[i];
    // 任一行形状不符 → 整表按未翻译处理（回落原文，不显示错位内容）
    if (!Array.isArray(row) || row.length !== grid.rows[i].cells.length) return null;
    matrix.push(row.map((cell) => (typeof cell === "string" ? cell : null)));
  }
  return matrix;
}

/** 单元格显示文本：译文优先，null（与目标语言相同）/缺译文回落原文 */
export function cellText(grid: TableGrid, matrix: TableMatrix | null, row: number, col: number): string {
  const translated = matrix?.[row]?.[col];
  return typeof translated === "string" ? translated : grid.rows[row].cells[col].text;
}

/** 表格 → 纯文本（原文栏悬停卡用：每行一条，" | " 分隔单元格） */
export function tableToText(grid: TableGrid, matrix: TableMatrix | null): string {
  return grid.rows
    .map((row, i) =>
      row.cells
        .map((_, j) => cellText(grid, matrix, i, j))
        .filter((t) => t.trim().length > 0)
        .join(" | "),
    )
    .filter((line) => line.length > 0)
    .join("\n");
}
