/** Truth is Rust; bindings are ts-rs output (run `cargo test`, never hand-edit). */

/** Stable id minted at import; the backend resolves the path from (root, id). */
export type PDFId = string;

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

export interface RepoGroup {
  folder: string | null;
  pdfs: PDFStruct[];
}

export type LayoutMode = "ot" | "to" | "o" | "t";
