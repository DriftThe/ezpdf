/**
 * 版面块类型决策（PLAN-OCR.md §8.3，用户拍板 2026-09-11；formula 覆盖 2026-09-12；
 * footer/vision_footnote 送翻 2026-09-14；送翻类型改为用户可配 2026-09-15）：
 *
 * - 送翻类型（用户可在 设置→常规 勾选）：LLM 阶段送翻；
 *   改动只影响尚未翻译的页面（已翻页面的译文已在绑定 JSON 里）；
 * - 覆盖渲染类型（= 勾选的送翻类型 + formula）：译文栏渲染白底覆盖框
 *   （bypass 期 translation=null → 框内显示原文 content）；formula 不送翻，
 *   但用 KaTeX 覆盖渲染（原文像素直出会被白框盖掉，公式视觉无损）；
 * - 未勾选类型：不做任何覆盖——原 PDF 像素直出，OCR content 仅存于绑定 JSON。
 *   image 不提供勾选（图片没有可翻译的文本）；table 自 2026-09-16 起提供勾选但不默认勾
 *   （表格走网格 JSON 送翻 + 网页表格渲染，见 lib/table.ts 与 reader/TableCover.vue）；
 *   其余类型都可被勾选，默认勾选 DEFAULT_TRANSLATED_TYPES。
 */

/** 可勾选的送翻类型：PP-DocLayoutV3 label 原样字符串；顺序即设置页展示顺序（默认在前） */
export const BLOCK_TYPE_OPTIONS: readonly string[] = [
  // 默认送翻
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
  // 可选送翻（默认不勾）
  "reference",
  "reference_content",
  "algorithm",
  "number",
  "header",
  "chart",
  "seal",
  "table",
];

/** 默认勾选项数 = BLOCK_TYPE_OPTIONS 开头「默认送翻」那一段的长度 */
const DEFAULT_TYPE_COUNT = 10;

/** 默认送翻集合 = 上面列表的前 N 项（同一份字面量，避免同一份清单写两遍；
 *  与 Rust translate.rs 的 TRANSLATABLE_TYPES 同步——那边是跨语言副本，改这里要一起改） */
export const DEFAULT_TRANSLATED_TYPES: readonly string[] =
  BLOCK_TYPE_OPTIONS.slice(0, DEFAULT_TYPE_COUNT);

/** 公式类型：始终覆盖渲染（KaTeX），与是否送翻无关 */
const FORMULA_TYPE = "formula";

/** 该类型是否在译文栏渲染白底覆盖框（translated = 用户勾选的送翻类型集合） */
export function isOverlayType(kind: string, translated: ReadonlySet<string>): boolean {
  return kind === FORMULA_TYPE || translated.has(kind);
}
