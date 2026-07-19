import markdownit from "markdown-it";
import hljs from "highlight.js/lib/common";

// 注册本项目常用的语言（highlight.js 的 common 包已含 rust/bash/json 等，
// 这里额外确保 glsl 之类有兜底）。
hljs.registerAliases(["glsl", "vert", "frag", "slang"], { languageName: "cpp" });

// 自定义 fence 渲染：外层包裹带「语言标签 + 复制按钮」的工具条。
// 支持扩展语法：```rust id=foo file=swapchain.rs
function renderFence(
  tokens: any[],
  idx: number,
  _options: any,
  _env: any,
  _slf: any
): string {
  const token = tokens[idx];
  const info = token.info ? token.info.trim() : "";
  const parts = info.split(/\s+/);
  const lang = parts[0] || "text";
  const attrMap: Record<string, string> = {};
  for (const p of parts.slice(1)) {
    const m = /^(\w+)=(.+)$/.exec(p);
    if (m) attrMap[m[1]] = m[2];
  }
  const code = token.content;

  let highlighted: string;
  if (lang && hljs.getLanguage(lang)) {
    highlighted = hljs.highlight(code, { language: lang, ignoreIllegals: true })
      .value;
  } else {
    highlighted = hljs.highlightAuto(code).value;
  }

  // 按行拆成 .line（带行号、可高亮），用于「代码逐行联动」。
  const lines = highlighted.split("\n");
  // 末尾若为空行则去掉（split 常多出一行）
  if (lines.length > 1 && lines[lines.length - 1].trim() === "") lines.pop();
  const linesHtml = lines
    .map((l) => `<span class="line">${l || " "}</span>`)
    .join("");

  const langLabel = lang.toUpperCase();
  const blockId = attrMap.id ? ` id="cb-${attrMap.id}"` : "";
  const fname = attrMap.file
    ? `<span class="fname">${attrMap.file}</span>`
    : "";
  // 用 data-code 存原始代码便于复制（避免 HTML 实体转义问题）。
  return (
    `<div class="code-block"${blockId}>` +
    `<div class="code-head"><span class="lang">${langLabel}</span>${fname}` +
    `<button class="copy-btn" data-code="${encodeURIComponent(
      code
    )}">复制</button></div>` +
    `<pre><code class="hljs language-${lang}">${linesHtml}</code></pre>` +
    `</div>`
  );
}

export const md = markdownit({
  html: false,
  linkify: true,
  typographer: false,
  highlight: (str: string, lang: string): string => {
    // 占位：实际高亮在 renderFence 中完成（markdown-it 在 fence 规则调用 highlight）。
    if (lang && hljs.getLanguage(lang)) {
      try {
        return hljs.highlight(str, { language: lang, ignoreIllegals: true })
          .value;
      } catch {
        /* fall through */
      }
    }
    return ""; // 返回空，让 renderFence 兜底
  },
});

// 覆盖默认的 fence 渲染器，注入工具条。
md.renderer.rules.fence = renderFence;

// 给文章标题自动生成锚点 id（供右侧 TOC 跳转）。
// 中文标题保留，用 "-" 连接；重复 id 自动加序号。
const usedHeadingIds = new Set<string>();
const slugifyHeading = (s: string): string =>
  s
    .trim()
    .toLowerCase()
    .replace(/[^\w一-龥]+/g, "-")
    .replace(/^-+|-+$/g, "");

md.renderer.rules.heading_open = (tokens, idx, options, _env, self) => {
  const inline = tokens[idx + 1];
  const text = inline?.content ?? "";
  let base = slugifyHeading(text) || `section-${idx}`;
  let id = base;
  let n = 1;
  while (usedHeadingIds.has(id)) id = `${base}-${++n}`;
  usedHeadingIds.add(id);
  tokens[idx].attrSet("id", id);
  return self.renderToken(tokens, idx, options);
};

// 让 :::tip / :::warn / :::danger / :::info 容器语法可用。
// 解析形如：:::tip 标题\n内容\n:::
const containerPlugin = (mdLocal: typeof md) => {
  const block = (
    state: any,
    startLine: number,
    endLine: number,
    silent: boolean
  ) => {
    const start = state.bMarks[startLine] + state.tShift[startLine];
    const max = state.eMarks[startLine];
    const lineText = state.src.slice(start, max).trim();
    const m = /^:::\s*(tip|warn|danger|info)\s*(.*)$/.exec(lineText);
    if (!m) return false;
    if (silent) return false;

    const kind = m[1];
    const title = m[2] || "";
    const lines: string[] = [];
    let nextLine = startLine + 1;
    while (nextLine < endLine) {
      const s = state.bMarks[nextLine] + state.tShift[nextLine];
      const e = state.eMarks[nextLine];
      const t = state.src.slice(s, e).trim();
      if (t === ":::") break;
      lines.push(state.src.slice(s, e));
      nextLine++;
    }

    const inner = lines.join("\n");
    const inlineTitle = title
      ? `<span class="label">${mdLocal.renderInline(title)}</span>`
      : `<span class="label">${
          kind === "tip"
            ? "提示"
            : kind === "warn"
            ? "注意"
            : kind === "danger"
            ? "警告"
            : "说明"
        }</span>`;

    const token = state.push("html_block", "", 0);
    token.content = `<div class="callout ${kind}">${inlineTitle}<p>${mdLocal.render(
      inner
    )}</p></div>`;
    token.map = [startLine, nextLine + 1];
    state.line = nextLine + 1;
    return true;
  };
  mdLocal.block.ruler.before("fence", "callout", block, {
    alt: ["paragraph", "reference", "blockquote", "list"],
  });
};

// 让 :::exercise\n内容\n::: 渲染为练习框。
const exercisePlugin = (mdLocal: typeof md) => {
  const block = (
    state: any,
    startLine: number,
    endLine: number,
    silent: boolean
  ) => {
    const start = state.bMarks[startLine] + state.tShift[startLine];
    const max = state.eMarks[startLine];
    const lineText = state.src.slice(start, max).trim();
    if (!/^:::exercise\s*$/.test(lineText)) return false;
    if (silent) return false;
    const lines: string[] = [];
    let nextLine = startLine + 1;
    while (nextLine < endLine) {
      const s = state.bMarks[nextLine] + state.tShift[nextLine];
      const e = state.eMarks[nextLine];
      const t = state.src.slice(s, e).trim();
      if (t === ":::") break;
      lines.push(state.src.slice(s, e));
      nextLine++;
    }
    const inner = mdLocal.render(lines.join("\n"));
    const token = state.push("html_block", "", 0);
    token.content = `<div class="exercise"><span class="label">动手练习</span>${inner}</div>`;
    token.map = [startLine, nextLine + 1];
    state.line = nextLine + 1;
    return true;
  };
  mdLocal.block.ruler.before("fence", "exercise", block, {
    alt: ["paragraph", "reference", "blockquote", "list"],
  });
};

containerPlugin(md);
exercisePlugin(md);

// 渲染工具：把 markdown 字符串转成 HTML，并在渲染后绑定复制按钮事件。
export function renderMarkdown(src: string): string {
  usedHeadingIds.clear();
  return md.render(src);
}

export function bindCopyButtons(root: HTMLElement): void {
  root.querySelectorAll<HTMLButtonElement>(".copy-btn").forEach((btn) => {
    btn.addEventListener("click", () => {
      const data = btn.getAttribute("data-code");
      if (!data) return;
      const code = decodeURIComponent(data);
      navigator.clipboard?.writeText(code).then(() => {
        const old = btn.textContent;
        btn.textContent = "已复制 ✓";
        setTimeout(() => (btn.textContent = old), 1400);
      });
    });
  });
}

// 代码逐行联动：高亮指定代码块（id 不含 cb- 前缀）的某些行并滚动进视口。
import { flashLines } from "./anim";
export function highlightLines(blockId: string, ranges: [number, number][]): void {
  // 支持传 "foo" 或 "cb-foo"
  const el = document.getElementById(
    blockId.startsWith("cb-") ? blockId : `cb-${blockId}`
  );
  if (!el) return;
  const codeEl = el.querySelector("code");
  if (!codeEl) return;
  const lineEls = Array.from(codeEl.querySelectorAll<HTMLElement>(".line"));
  // 先清旧高亮
  lineEls.forEach((l) => l.classList.remove("hl"));
  const toFlash: HTMLElement[] = [];
  for (const [a, b] of ranges) {
    for (let i = a; i <= b; i++) {
      const l = lineEls[i - 1];
      if (l) {
        l.classList.add("hl");
        toFlash.push(l);
      }
    }
  }
  el.scrollIntoView({ behavior: "smooth", block: "center" });
  flashLines(toFlash);
}
