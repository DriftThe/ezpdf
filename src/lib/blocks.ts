/** Cover-rendered = checked types + formula (KaTeX); unchecked keep original pixels. */

/** Selectable types: raw PP-DocLayoutV3 labels; this order is the settings order. */
export const BLOCK_TYPE_OPTIONS: readonly string[] = [
  // default on
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
  // optional (off by default)
  "reference",
  "reference_content",
  "algorithm",
  "number",
  "header",
  "chart",
  "seal",
  "table",
];

const DEFAULT_TYPE_COUNT = 10;

/** First N entries above; keep in sync with translate.rs TRANSLATABLE_TYPES. */
export const DEFAULT_TRANSLATED_TYPES: readonly string[] =
  BLOCK_TYPE_OPTIONS.slice(0, DEFAULT_TYPE_COUNT);

const FORMULA_TYPE = "formula";

export function isOverlayType(kind: string, translated: ReadonlySet<string>): boolean {
  return kind === FORMULA_TYPE || translated.has(kind);
}
