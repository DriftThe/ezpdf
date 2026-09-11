/**
 * 版面块类型决策（PLAN-OCR.md §8.3，用户拍板 2026-09-11）：
 *
 * - 送翻类型：LLM 阶段送翻；译文栏渲染白底覆盖框（bypass 期 translation=null →
 *   框内显示原文 content）；
 * - 其余类型（table/chart/image/formula/number/header/footer/seal/algorithm/
 *   reference/reference_content）：不送翻且**不做任何覆盖**——原 PDF 像素直出，
 *   OCR content 仅存于绑定 JSON 供后续阶段（表格单元格翻译、KaTeX 等）取用。
 */

/** 送翻类型集合：PP-DocLayoutV3 label 原样字符串（string 通道，未知新标签默认不送翻） */
export const TRANSLATED_TYPES: ReadonlySet<string> = new Set([
  "text",
  "paragraph_title",
  "doc_title",
  "abstract",
  "aside_text",
  "footnote",
  "figure_title",
  "content",
]);

/** 该类型是否送翻（= 译文栏是否渲染白底覆盖框） */
export function isTranslatedType(kind: string): boolean {
  return TRANSLATED_TYPES.has(kind);
}
