import katex from "katex";

/**
 * v-html safety: KaTeX output is trusted, everything else escaped. Newlines become <br>
 * (not pre-line, which would split KaTeX internals); inline $ must touch non-space.
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
