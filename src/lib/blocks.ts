/**
 * Block-type policy:
 * - Checked types (Settings → Common) go to the LLM; changes only affect untranslated pages.
 * - Cover-rendered types = checked types + formula: the translation pane draws a white box
 *   (translation=null, i.e. bypass, shows the source content); formula is never sent but is
 *   KaTeX-rendered so the white box does not hide the original pixels.
 * - Unchecked types get no cover — original pixels only; OCR text stays in the bound JSON.
 *   image is not selectable; table is selectable but off by default (grid JSON + TableCover).
 */

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

/** Number of default-on entries = the leading "default on" run of BLOCK_TYPE_OPTIONS. */
const DEFAULT_TYPE_COUNT = 10;

/** First N entries of the list above; keep in sync with translate.rs TRANSLATABLE_TYPES. */
export const DEFAULT_TRANSLATED_TYPES: readonly string[] =
  BLOCK_TYPE_OPTIONS.slice(0, DEFAULT_TYPE_COUNT);

/** Formula type: always cover-rendered with KaTeX, independent of translation. */
const FORMULA_TYPE = "formula";

/** Whether the type gets a white cover box (translated = the user's checked set). */
export function isOverlayType(kind: string, translated: ReadonlySet<string>): boolean {
  return kind === FORMULA_TYPE || translated.has(kind);
}
