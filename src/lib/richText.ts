import katex from "katex";

/**
 * 覆盖框富文本渲染：把 OCR 文本里的 LaTeX 公式段交给 KaTeX，其余原文 HTML 转义
 * （v-html 的安全前提：只有 KaTeX 生成的 HTML 可信，其余字符一律实体化）。
 * 支持 $$…$$ / \[…\]（块级），$…$ / \(…\)（行内）；行内 $ 要求内侧紧邻非空白，
 * 避免把 "$5 … $" 这类货币文本误当公式。
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

/** 文本 → 可安全 v-html 的 HTML（公式 KaTeX，正文转义） */
export function renderRichText(raw: string): string {
  let html = "";
  let cursor = 0;
  for (const m of raw.matchAll(MATH_RE)) {
    const start = m.index ?? 0;
    html += escapeHtml(raw.slice(cursor, start));
    const display = m[1] !== undefined || m[2] !== undefined;
    const expr = m[1] ?? m[2] ?? m[3] ?? m[4] ?? "";
    html += katex.renderToString(expr, {
      displayMode: display,
      throwOnError: false,
      strict: false,
    });
    cursor = start + m[0].length;
  }
  return html + escapeHtml(raw.slice(cursor));
}
