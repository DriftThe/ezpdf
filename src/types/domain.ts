/**
 * ezpdf 域模型 — 与 PLAN.md 的 ezpdf 格式规范对齐。
 * 前端侧镜像定义，后续与 Rust serde 结构一一对应（阶段1）。
 */

/** 书 id = 书文件夹绝对路径（与 Rust 端约定一致） */
export type BookId = string;

/** 页解析状态机：pending → ocr_queued → ocr_done → translating → done / failed */
export type PageStatus =
  | "pending"
  | "ocr_queued"
  | "ocr_done"
  | "translating"
  | "done"
  | "failed";

/** PP-DocLayoutV3 版面标签；保留 string 通道以兼容后续新增标签 */
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

/** [x, y, w, h]，单位 PDF 点（1pt = 1/72in），左上原点，scale=1 视口（与缩放无关） */
export type BboxPt = [number, number, number, number];

export interface Block {
  id: string; // 如 "p3-b12"
  label: BlockLabel;
  bboxPt: BboxPt;
  score: number;
  /** markdown 原文：正文纯文本 / 公式 $$..$$ / 表格 markdown；figure 块为 null */
  source: string | null;
  /** markdown 译文；figure/formula 块为 null（formula 原样渲染） */
  translation: string | null;
}

export interface PageInfo {
  index: number; // 0-based
  status: PageStatus;
  widthPt: number;
  heightPt: number;
  blocks: Block[];
}

export interface BookMeta {
  name: string;
  pageCount: number;
  createdAt: string;
  updatedAt: string;
  targetLang: string;
  ocrEngine: string;
  llmModel: string;
}

export interface Book {
  id: BookId;
  name: string;
  pdfPath: string;
  jsonPath: string;
  meta: BookMeta;
  pages: PageInfo[];
}

/** 后端 ts-rs 导出的绑定（src-tauri/bindings/；struct 变更后跑 `cargo test` 重新生成，勿手改） */
export type { PDFStruct } from "../../src-tauri/bindings/PDFStruct";
export type { RepoTree } from "../../src-tauri/bindings/RepoTree";

/** 仓库树的 UI 递归视图模型，由 buildRepoNodes 从后端平铺索引 RepoTree 转换而来（v1 无子文件夹，但保留递归结构以便将来恢复）。
 *  book.path = 书目录绝对路径，即书的唯一索引（belong 目录 + 书名） */
export type RepoNode =
  | { type: "folder"; name: string; path: string; children: RepoNode[] }
  | { type: "book"; name: string; path: BookId; /** 绑定的结构 JSON（仓库相对路径）；null = 未解析 */ bind: string | null };

export type Mode = "repo" | "offline";

/** 阅读器布局 4 态：原│译 / 译│原 / 仅原 / 仅译 */
export type LayoutMode = "ot" | "to" | "o" | "t";

export type ServiceStatus = "unknown" | "starting" | "connected" | "disconnected";

/** 译文栏允许白底覆盖并重渲染的块（figure 与未识别区域保持原样） */
export function isRenderableLabel(label: BlockLabel): boolean {
  return label !== "figure";
}
