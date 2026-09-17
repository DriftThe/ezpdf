/**
 * Table rendering side. Rust (src-tauri/src/table.rs) parses the markup into `block.grid`
 * and stores translations as a 2-D matrix JSON in `block.translation`; this module only
 * interprets both, so there is a single parser and no column-mapping drift.
 * `grid.rows[i].cells[j]` are real cells (`<lcel>` folded into `colspan`); the matrix lines
 * up 1:1 with them, and null (same as the target language) falls back to the source.
 */

import type { TableGrid } from "../types/domain";

/** Translation matrix rows × real cells; null = same as target → render the source. */
type TableMatrix = (string | null)[][];

/** Parse block.translation matrix JSON; invalid or shape mismatch → null. */
export function parseTableMatrix(translation: string | null, grid: TableGrid): TableMatrix | null {
  if (!translation) return null;
  let raw: unknown;
  try {
    raw = JSON.parse(translation);
  } catch {
    return null;
  }
  // tolerate a {"table": [[...]]} wrapper (prompt wants a bare array; models add shells)
  const rows = Array.isArray(raw)
    ? raw
    : raw && typeof raw === "object" && Array.isArray((raw as { table?: unknown }).table)
      ? ((raw as { table: unknown[] }).table)
      : null;
  if (!rows || rows.length !== grid.rows.length) return null;
  const matrix: TableMatrix = [];
  for (let i = 0; i < rows.length; i++) {
    const row = rows[i];
    // any row shape mismatch → treat the whole table as untranslated (fall back to source)
    if (!Array.isArray(row) || row.length !== grid.rows[i].cells.length) return null;
    matrix.push(row.map((cell) => (typeof cell === "string" ? cell : null)));
  }
  return matrix;
}

/** Cell text: translation first, null/missing → source. */
function cellText(grid: TableGrid, matrix: TableMatrix | null, row: number, col: number): string {
  const translated = matrix?.[row]?.[col];
  return typeof translated === "string" ? translated : grid.rows[row].cells[col].text;
}

/** Table → plain text for the hover card: one line per row, cells joined by " | ". */
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
