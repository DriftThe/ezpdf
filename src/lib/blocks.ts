/**
 * 版面块类型决策（PLAN-OCR.md §8.3，用户拍板 2026-09-11；formula 覆盖 2026-09-12；
 * footer/vision_footnote 送翻 2026-09-14）：
 *
 * - 送翻类型（TRANSLATED_TYPES）：LLM 阶段送翻；
 * - 覆盖渲染类型（OVERLAY_TYPES = 送翻 + formula）：译文栏渲染白底覆盖框
 *   （bypass 期 translation=null → 框内显示原文 content）；formula 不送翻，
 *   但用 KaTeX 覆盖渲染（原文像素直出会被白框盖掉，公式视觉无损）；
 * - 其余类型（table/chart/image/number/header/seal/algorithm/
 *   reference/reference_content）：不做任何覆盖——原 PDF 像素直出，
 *   OCR content 仅存于绑定 JSON 供后续阶段（表格单元格翻译等）取用。
 */

/** 送翻类型集合：PP-DocLayoutV3 label 原样字符串（string 通道，未知新标签默认不送翻） */
export const TRANSLATED_TYPES: ReadonlySet<string> = new Set([
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
]);

/** 覆盖渲染类型集合：送翻类型 + formula（公式由 richText KaTeX 渲染） */
export const OVERLAY_TYPES: ReadonlySet<string> = new Set([...TRANSLATED_TYPES, "formula"]);

/** 该类型是否在译文栏渲染白底覆盖框 */
export function isOverlayType(kind: string): boolean {
  return OVERLAY_TYPES.has(kind);
}
