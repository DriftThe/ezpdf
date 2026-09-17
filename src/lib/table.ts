/** Rust alone parses tables (grid + 2-D matrix translation); this module only interprets both. */

import type { TableGrid } from "../types/domain";

type TableMatrix = (string | null)[][];

export function parseTableMatrix(translation: string | null, grid: TableGrid): TableMatrix | null {
  if (!translation) return null;
  let raw: unknown;
  try {
    raw = JSON.parse(translation);
  } catch {
    return null;
  }
  // tolerate a {"table": [...]} wrapper (models add shells)
  const rows = Array.isArray(raw)
    ? raw
    : raw && typeof raw === "object" && Array.isArray((raw as { table?: unknown }).table)
      ? ((raw as { table: unknown[] }).table)
      : null;
  if (!rows || rows.length !== grid.rows.length) return null;
  const matrix: TableMatrix = [];
  for (let i = 0; i < rows.length; i++) {
    const row = rows[i];
    if (!Array.isArray(row) || row.length !== grid.rows[i].cells.length) return null;
    matrix.push(row.map((cell) => (typeof cell === "string" ? cell : null)));
  }
  return matrix;
}

function cellText(grid: TableGrid, matrix: TableMatrix | null, row: number, col: number): string {
  const translated = matrix?.[row]?.[col];
  return typeof translated === "string" ? translated : grid.rows[row].cells[col].text;
}

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
