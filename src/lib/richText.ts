import katex from "katex";

/**
 * Rich-text rendering for cover boxes: LaTeX segments go to KaTeX, everything else is
 * HTML-escaped (v-html safety: only KaTeX output is trusted).
 * Supports $$…$$ / \[…\] (display) and $…$ / \(…\) (inline); inline $ must be adjacent
 * to non-space so "$5 … $" is not mistaken for math.
 * Newlines become <br> (models insert \n in structured blocks; HTML collapses them).
 * Not white-space: pre-line, which would also split KaTeX's own internal newlines.
 */

const ESCAPES: Record<string, string> = {
  "&": "&amp;",
  "<": "&lt;",
  ">": "&gt;",
  '"': "&quot;",
  "'": "&#39;",
};

function escapeHtml(text: string): string {
  return text.replace(/[&<>"']/g, (c) => ESCAPES[c]);
}

const MATH_RE =
  /\$\$([\s\S]+?)\$\$|\\\[([\s\S]+?)\\\]|\$(\S(?:[^$\n]*\S)?)\$|\\\(([\s\S]+?)\\\)/g;

function textToHtml(text: string): string {
  return escapeHtml(text).replace(/\n/g, "<br>");
}

/** Text → safe-for-v-html HTML (KaTeX math, escaped body, newlines as <br>). */
export function renderRichText(raw: string): string {
  let html = "";
  let cursor = 0;
  for (const m of raw.matchAll(MATH_RE)) {
    const start = m.index ?? 0;
    html += textToHtml(raw.slice(cursor, start));
    const display = m[1] !== undefined || m[2] !== undefined;
    const expr = m[1] ?? m[2] ?? m[3] ?? m[4] ?? "";
    html += katex.renderToString(expr, {
      displayMode: display,
      throwOnError: false,
      strict: false,
    });
    cursor = start + m[0].length;
  }
  return html + textToHtml(raw.slice(cursor));
}
