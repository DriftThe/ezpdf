/**
 * ezpdf 域模型 — 真相源在 Rust 端（src-tauri/src/lib.rs），经 ts-rs 导出到 src-tauri/bindings/；
 * struct 变更后跑 `cargo test` 重新生成绑定，勿手改绑定文件。
 * 本文件只保留：前端内部标识（PDFId/pdfIndexKey）、UI 视图模型（RepoGroup）与 UI 常量（BLOCK_LABELS 等）。
 */

/** PDF id = 索引键（pdfIndexKey：belong/name，根级为 name）——由 .ezrepo 托管，不是物理路径；
 *  物理路径由后端凭 (root, belong, name) 解析后下发 */
export type PDFId = string;

/** PP-DocLayoutV3 版面标签集合；Rust 端 Block.label 为 string（保留通道兼容后续新增标签），此处仅约束 UI 已知集合 */
export const BLOCK_LABELS = [
  "text",
  "title",
  "list",
  "figure",
  "figure_caption",
  "table",
  "formula",
  "header",
  "footer",
] as const;
export type BlockLabel = (typeof BLOCK_LABELS)[number] | (string & {});

/** 后端 ts-rs 导出的绑定（src-tauri/bindings/） */
import type { PDFStruct } from "../../src-tauri/bindings/PDFStruct";
export type { PageStatus } from "../../src-tauri/bindings/PageStatus";
export type { BboxPt } from "../../src-tauri/bindings/BboxPt";
export type { Block } from "../../src-tauri/bindings/Block";
export type { PageInfo } from "../../src-tauri/bindings/PageInfo";
export type { PDFMeta } from "../../src-tauri/bindings/PDFMeta";
export type { PDF } from "../../src-tauri/bindings/PDF";
export type { PDFStruct } from "../../src-tauri/bindings/PDFStruct";
export type { RepoTree } from "../../src-tauri/bindings/RepoTree";

/** 仓库索引的 UI 分组视图：按 belong 平铺分组（v1 无子文件夹），由 library store 的 repoGroups 从平铺索引派生 */
export interface RepoGroup {
  /** 所属目录名；null = 根级（belong 为空的 PDF 直接挂在仓库根部） */
  folder: string | null;
  pdfs: PDFStruct[];
}

/** PDF 在索引中的唯一键：belong/name；根级（belong 为空）为 name。
 *  仅作前端内部标识与 pdfs 映射键；向后端查询时始终携带完整索引项（name/belong/bind）。 */
export function pdfIndexKey(pdf: Pick<PDFStruct, "name" | "belong">): string {
  const belong = pdf.belong?.trim();
  return belong ? `${belong}/${pdf.name}` : pdf.name;
}

export type Mode = "repo" | "offline";

/** 阅读器布局 4 态：原│译 / 译│原 / 仅原 / 仅译 */
export type LayoutMode = "ot" | "to" | "o" | "t";

export type ServiceStatus = "unknown" | "starting" | "connected" | "disconnected";

/** 译文栏允许白底覆盖并重渲染的块（figure 与未识别区域保持原样） */
export function isRenderableLabel(label: BlockLabel): boolean {
  return label !== "figure";
}
