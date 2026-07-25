import "./styles.css";
import { CHAPTERS, findChapter, type ChapterMeta } from "./chapters";
import { renderMarkdown, bindCopyButtons } from "./highlight";
import { mountViz } from "./viz/index";
import {
  animateChapterSwitch,
  animateVizIn,
  pulseSidebar,
} from "./anim";
import introMd from "./content/01-intro.md?raw";
import helloMd from "./content/02-hello.md?raw";
import depsMd from "./content/03-deps.md?raw";
import winitMd from "./content/04-winit.md?raw";
import contextMd from "./content/05-context.md?raw";
import swapchainMd from "./content/06-swapchain.md?raw";
import pipelineMd from "./content/07-pipeline.md?raw";
import ecsMd from "./content/08-ecs.md?raw";
import ecsRenderMd from "./content/09-ecs-render.md?raw";
import assetsMd from "./content/10-assets.md?raw";
import pbrMd from "./content/11-pbr.md?raw";
import androidMd from "./content/12-android.md?raw";
import reviewMd from "./content/13-review.md?raw";
import renderAdvancedMd from "./content/14-render-advanced.md?raw";
import engineToolsMd from "./content/15-engine-tools.md?raw";

const CONTENT: Record<string, string> = {
  intro: introMd,
  hello: helloMd,
  deps: depsMd,
  winit: winitMd,
  context: contextMd,
  swapchain: swapchainMd,
  pipeline: pipelineMd,
  ecs: ecsMd,
  "ecs-render": ecsRenderMd,
  assets: assetsMd,
  pbr: pbrMd,
  android: androidMd,
  review: reviewMd,
  "render-advanced": renderAdvancedMd,
  "engine-tools": engineToolsMd,
};

const app = document.getElementById("app")!;
let sidebarOpen = false;

function groupChapters(): Record<string, ChapterMeta[]> {
  const groups: Record<string, ChapterMeta[]> = {};
  for (const c of CHAPTERS) {
    (groups[c.group] ??= []).push(c);
  }
  return groups;
}

function renderShell() {
  const groups = groupChapters();
  const groupOrder = ["基础", "图形", "引擎"];
  const sidebarInner = groupOrder
    .map((g) => {
      const items = (groups[g] ?? [])
        .map((c) => {
          const num = c.title.split("·")[0].trim();
          return `<a class="chapter-link" data-id="${c.id}">
            <div class="t"><span class="num">${num}</span>${c.title
            .split("·")[1]
            .trim()}</div>
            <div class="s">${c.subtitle}</div>
          </a>`;
        })
        .join("");
      return `<div class="side-title">${g}</div>${items}`;
    })
    .join("");

  app.innerHTML = `
    <div class="topbar">
      <button class="menu-btn" id="menu-btn">☰</button>
      <div class="brand">Prisma<span>Rev</span></div>
      <span class="chapter-counter" id="chapter-counter"></span>
      <span class="ver-tag" title="教程基准 git 提交">0b48449</span>
    </div>
    <aside class="sidebar" id="sidebar">${sidebarInner}</aside>
    <div class="sidebar-backdrop" id="backdrop"></div>
    <main class="content">
      <article class="article" id="article"></article>
      <nav class="pager" id="pager"></nav>
    </main>
    <button class="toc-btn" id="toc-btn" aria-label="本页目录">
      <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
        <path d="M3 3.5h10M3 8h10M3 12.5h10"/>
        <circle cx="3" cy="3.5" r="1.2" fill="currentColor" stroke="none"/>
        <circle cx="3" cy="8" r="1.2" fill="currentColor" stroke="none"/>
        <circle cx="3" cy="12.5" r="1.2" fill="currentColor" stroke="none"/>
      </svg>
    </button>
    <aside class="toc-panel" id="toc-panel">
      <div class="toc-head"><span>本页目录</span></div>
      <div class="toc-body" id="toc-body"></div>
    </aside>
  `;

  const sidebar = document.getElementById("sidebar")!;
  const backdrop = document.getElementById("backdrop")!;

  document.getElementById("menu-btn")!.addEventListener("click", () => {
    sidebarOpen = !sidebarOpen;
    sidebar.classList.toggle("open", sidebarOpen);
    backdrop.classList.toggle("show", sidebarOpen);
  });
  backdrop.addEventListener("click", () => {
    sidebarOpen = false;
    sidebar.classList.remove("open");
    backdrop.classList.remove("show");
  });

  sidebar.querySelectorAll<HTMLElement>(".chapter-link").forEach((el) => {
    el.addEventListener("click", () => {
      const id = el.getAttribute("data-id")!;
      location.hash = `#/${id}`;
      sidebarOpen = false;
      sidebar.classList.remove("open");
      backdrop.classList.remove("show");
    });
  });

  /* 浮动 TOC 按钮：点击切换面板 */
  const tocBtn = document.getElementById("toc-btn")!;
  const tocPanel = document.getElementById("toc-panel")!;
  tocBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    tocPanel.classList.toggle("open");
  });
  document.addEventListener("click", (e) => {
    if (
      !tocPanel.contains(e.target as Node) &&
      !tocBtn.contains(e.target as Node)
    ) {
      tocPanel.classList.remove("open");
    }
  });
}

function setActive(id: string) {
  document.querySelectorAll<HTMLElement>(".chapter-link").forEach((el) => {
    const active = el.getAttribute("data-id") === id;
    el.classList.toggle("active", active);
    if (active) pulseSidebar(el);
  });
}

function renderPager(current: ChapterMeta) {
  const idx = CHAPTERS.indexOf(current);
  const prev = CHAPTERS[idx - 1];
  const next = CHAPTERS[idx + 1];
  const pager = document.getElementById("pager")!;
  const prevHtml = prev
    ? `<a class="prev" href="#/${prev.id}"><span class="dir">← 上一章</span><span class="ttl">${prev.title}</span></a>`
    : `<span></span>`;
  const nextHtml = next
    ? `<a class="next" href="#/${next.id}"><span class="dir">下一章 →</span><span class="ttl">${next.title}</span></a>`
    : `<span></span>`;
  pager.innerHTML = prevHtml + nextHtml;
}

function updateChapterCounter(current: ChapterMeta) {
  const idx = CHAPTERS.indexOf(current);
  const counter = document.getElementById("chapter-counter");
  if (counter) {
    counter.textContent = `第 ${idx + 1} 章 / 共 ${CHAPTERS.length} 章`;
  }
}

function renderChapter(id: string) {
  const chapter = findChapter(id) ?? CHAPTERS[0];
  const raw = CONTENT[chapter.id] ?? "# 内容缺失";
  const article = document.getElementById("article")!;
  article.innerHTML = renderMarkdown(raw);
  bindCopyButtons(article);

  if (chapter.viz && chapter.viz.length) {
    for (const vk of chapter.viz) {
      const vizHost = document.createElement("div");
      vizHost.className = "viz-host";
      article.appendChild(vizHost);
      mountViz(vk, vizHost);
      animateVizIn(vizHost);
    }
  }

  setActive(chapter.id);
  renderPager(chapter);
  updateChapterCounter(chapter);
  buildToc();
  // 先同步归零，待布局回流稳定后再确保一次，避免内容高度变化导致没回顶
  window.scrollTo(0, 0);
  requestAnimationFrame(() => window.scrollTo(0, 0));
  animateChapterSwitch(article);
}

// 浮动 TOC：从文章 h2/h3 生成锚点链接，滚动高亮当前节。
let tocScrollHandler: (() => void) | null = null;
function buildToc(): void {
  const article = document.getElementById("article")!;
  const toc = document.getElementById("toc-panel")!;
  const tocBtn = document.getElementById("toc-btn")!;
  const body = document.getElementById("toc-body")!;
  const heads = Array.from(
    article.querySelectorAll<HTMLElement>("h2, h3")
  );
  if (!heads.length) {
    toc.style.display = "none";
    tocBtn.style.display = "none";
    return;
  }
  toc.style.display = "";
  tocBtn.style.display = "";
  body.innerHTML = heads
    .map((h) => {
      const sub = h.tagName === "H3" ? " sub" : "";
      const text = (h.textContent ?? "").trim();
      return `<a class="toc-link${sub}" data-target="${h.id}" href="javascript:void(0)">${text}</a>`;
    })
    .join("");

  body.querySelectorAll<HTMLAnchorElement>(".toc-link").forEach((a) => {
    a.addEventListener("click", (e) => {
      e.preventDefault();
      const el = document.getElementById(a.dataset.target!);
      el?.scrollIntoView({ behavior: "smooth", block: "start" });
      toc.classList.remove("open");
    });
  });

  // 滚动时高亮当前所在小节（不污染章节路由的 location.hash）
  if (tocScrollHandler) window.removeEventListener("scroll", tocScrollHandler);
  tocScrollHandler = () => {
    let currentId = heads[0]?.id ?? "";
    for (const h of heads) {
      if (h.getBoundingClientRect().top <= 130) currentId = h.id;
      else break;
    }
    body.querySelectorAll<HTMLElement>(".toc-link").forEach((l) => {
      l.classList.toggle("active", l.dataset.target === currentId);
    });
  };
  window.addEventListener("scroll", tocScrollHandler, { passive: true });
  tocScrollHandler();
}

function route() {
  const hash = location.hash.replace(/^#\/?/, "");
  const id = CHAPTERS.some((c) => c.id === hash) ? hash : CHAPTERS[0].id;
  renderChapter(id);
}

function main() {
  renderShell();
  window.addEventListener("hashchange", route);
  route();
}

main();
