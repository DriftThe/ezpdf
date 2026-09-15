/**
 * 版面块类型决策（PLAN-OCR.md §8.3，用户拍板 2026-09-11；formula 覆盖 2026-09-12；
 * footer/vision_footnote 送翻 2026-09-14；送翻类型改为用户可配 2026-09-15）：
 *
 * - 送翻类型（用户可在 设置→常规 勾选，默认全选）：LLM 阶段送翻；
 *   改动只影响尚未翻译的页面（已翻页面的译文已在绑定 JSON 里）；
 * - 覆盖渲染类型（= 勾选的送翻类型 + formula）：译文栏渲染白底覆盖框
 *   （bypass 期 translation=null → 框内显示原文 content）；formula 不送翻，
 *   但用 KaTeX 覆盖渲染（原文像素直出会被白框盖掉，公式视觉无损）；
 * - 未勾选类型与其余类型（table/chart/image/number/header/seal/algorithm/
 *   reference/reference_content）：不做任何覆盖——原 PDF 像素直出，
 *   OCR content 仅存于绑定 JSON 供后续阶段（表格单元格翻译等）取用。
 */

/** 可勾选的送翻类型：PP-DocLayoutV3 label 原样字符串；顺序即设置页展示顺序 */
export const BLOCK_TYPE_OPTIONS: readonly string[] = [
  "text",
  "paragraph_title",
  "doc_title",
  "abstract",
  "aside_text",
  "footnote",
  "footer",
  "vision_footnote",
  "figure_title",
  "content",
];

/** 默认送翻集合（与 Rust translate.rs 的 TRANSLATABLE_TYPES 同步） */
export const DEFAULT_TRANSLATED_TYPES: readonly string[] = BLOCK_TYPE_OPTIONS;

/** 公式类型：始终覆盖渲染（KaTeX），与是否送翻无关 */
export const FORMULA_TYPE = "formula";

/** 该类型是否在译文栏渲染白底覆盖框（translated = 用户勾选的送翻类型集合） */
export function isOverlayType(kind: string, translated: ReadonlySet<string>): boolean {
  return kind === FORMULA_TYPE || translated.has(kind);
}
