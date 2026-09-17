/** ezpdf domain model. Truth is Rust (src-tauri/src/lib.rs), exported by ts-rs to
 *  src-tauri/bindings/ (run `cargo test` after struct changes; never hand-edit bindings).
 *  This file keeps only frontend internals (PDFId) and UI view models (RepoGroup). */

/** Stable PDF id (the .ezrepo entry id, minted at import; survives file moves/renames).
 *  Every frontend key uses it; the backend resolves the physical path from (root, id). */
export type PDFId = string;

/** ts-rs bindings (src-tauri/bindings/). */
import type { PDFStruct } from "../../src-tauri/bindings/PDFStruct";
export type { Block } from "../../src-tauri/bindings/Block";
export type { PageInfo } from "../../src-tauri/bindings/PageInfo";
export type { PDF } from "../../src-tauri/bindings/PDF";
export type { RepoTree } from "../../src-tauri/bindings/RepoTree";
export type { ImportOutcome } from "../../src-tauri/bindings/ImportOutcome";
export type { ServiceStatus } from "../../src-tauri/bindings/ServiceStatus";
export type { OcrEnvReport } from "../../src-tauri/bindings/OcrEnvReport";
export type { ParseOutcome } from "../../src-tauri/bindings/ParseOutcome";
export type { ParsePageInput } from "../../src-tauri/bindings/ParsePageInput";
export type { InstallProgress } from "../../src-tauri/bindings/InstallProgress";
export type { ParseServiceHealth } from "../../src-tauri/bindings/ParseServiceHealth";
export type { TableGrid } from "../../src-tauri/bindings/TableGrid";

/** UI grouping view derived from the flat index by belong (v1 has no subfolders). */
export interface RepoGroup {
  /** Folder name; null = repo root. */
  folder: string | null;
  pdfs: PDFStruct[];
}

/** Layout: original│translation, translation│original, original only, translation only. */
export type LayoutMode = "ot" | "to" | "o" | "t";
