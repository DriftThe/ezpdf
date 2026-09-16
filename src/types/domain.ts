/**
 * ezpdf 域模型 — 真相源在 Rust 端（src-tauri/src/lib.rs），经 ts-rs 导出到 src-tauri/bindings/；
 * struct 变更后跑 `cargo test` 重新生成绑定，勿手改绑定文件。
 * 本文件只保留：前端内部标识（PDFId）与 UI 视图模型（RepoGroup）。
 */

/** PDF 的稳定唯一标识符（.ezrepo 条目的 id，导入时生成；文件挪动/改名不变）。
 *  前端一切键（pdfs 缓存、currentPdfId、页码记忆）都用它；物理路径由后端凭 (root, id) 查索引解析后下发 */
export type PDFId = string;

/** 后端 ts-rs 导出的绑定（src-tauri/bindings/） */
import type { PDFStruct } from "../../src-tauri/bindings/PDFStruct";
export type { Block } from "../../src-tauri/bindings/Block";
export type { PageInfo } from "../../src-tauri/bindings/PageInfo";
export type { PDF } from "../../src-tauri/bindings/PDF";
export type { PDFStruct } from "../../src-tauri/bindings/PDFStruct";
export type { RepoTree } from "../../src-tauri/bindings/RepoTree";
export type { ImportOutcome } from "../../src-tauri/bindings/ImportOutcome";
export type { ServiceStatus } from "../../src-tauri/bindings/ServiceStatus";
export type { OcrEnvReport } from "../../src-tauri/bindings/OcrEnvReport";
export type { ParseOutcome } from "../../src-tauri/bindings/ParseOutcome";
export type { ParsePageInput } from "../../src-tauri/bindings/ParsePageInput";
export type { InstallProgress } from "../../src-tauri/bindings/InstallProgress";
export type { ParseServiceHealth } from "../../src-tauri/bindings/ParseServiceHealth";
export type { TableGrid } from "../../src-tauri/bindings/TableGrid";
export type { TableRow } from "../../src-tauri/bindings/TableRow";
export type { TableCell } from "../../src-tauri/bindings/TableCell";

/** 仓库索引的 UI 分组视图：按 belong 平铺分组（v1 无子文件夹），由 library store 的 repoGroups 从平铺索引派生 */
export interface RepoGroup {
  /** 所属目录名；null = 根级（belong 为空的 PDF 直接挂在仓库根部） */
  folder: string | null;
  pdfs: PDFStruct[];
}

/** 阅读器布局 4 态：原│译 / 译│原 / 仅原 / 仅译 */
export type LayoutMode = "ot" | "to" | "o" | "t";
