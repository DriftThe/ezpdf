import katex from "katex";

/**
 * 覆盖框富文本渲染：把 OCR 文本里的 LaTeX 公式段交给 KaTeX，其余原文 HTML 转义
 * （v-html 的安全前提：只有 KaTeX 生成的 HTML 可信，其余字符一律实体化）。
 * 支持 $$…$$ / \[…\]（块级），$…$ / \(…\)（行内）；行内 $ 要求内侧紧邻非空白，
 * 避免把 "$5 … $" 这类货币文本误当公式。
 * 换行转成 <br>：模型会在目录这类结构化块里插 \n（提示词规则 5），而 HTML 默认把
 * 换行折叠成空格，覆盖框和悬浮卡片都得在这里显式还原（不用 white-space:pre-line，
 * 那会把 KaTeX 自带 HTML 里的换行也当换行，公式会被拆行）。
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

/** 文本 → 可安全 v-html 的 HTML（公式 KaTeX，正文转义，换行转 <br>） */
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
