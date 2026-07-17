import "./styles.css";
import { CHAPTERS, findChapter, type ChapterMeta } from "./chapters";
import { renderMarkdown, bindCopyButtons } from "./highlight";
import { mountViz } from "./viz/index";
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

// 章节 id -> 原始 markdown 内容
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
};

const app = document.getElementById("app")!;

let sidebarOpen = false;

function renderShell() {
  app.innerHTML = `
    <div class="topbar">
      <button class="menu-btn" id="menu-btn">☰</button>
      <div class="brand">Prisma<span>Rev</span><small>从 Rust 到 Vulkan 引擎 · 交互式教学</small><span class="ver-tag" title="教程基准 git 标签，对应源码快照">tutorial-v1</span></div>
      <div class="progress-wrap"><div class="progress-bar" id="progress-bar"></div></div>
      <div class="progress-label" id="progress-label">1 / ${CHAPTERS.length}</div>
    </div>
    <div class="layout">
      <aside class="sidebar" id="sidebar">
        <h2>学习路线</h2>
        ${CHAPTERS.map(
          (c) =>
            `<a class="chapter-link" data-id="${c.id}">
               <div class="t">${c.title}</div>
               <div class="s">${c.subtitle}</div>
             </a>`
        ).join("")}
      </aside>
      <div class="sidebar-backdrop" id="backdrop"></div>
      <main class="content">
        <article class="article" id="article"></article>
        <nav class="pager" id="pager"></nav>
      </main>
    </div>
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
}

function setActive(id: string) {
  document.querySelectorAll<HTMLElement>(".chapter-link").forEach((el) => {
    el.classList.toggle("active", el.getAttribute("data-id") === id);
  });
  const idx = CHAPTERS.findIndex((c) => c.id === id);
  const bar = document.getElementById("progress-bar")!;
  const label = document.getElementById("progress-label")!;
  if (idx >= 0) {
    bar.style.width = `${((idx + 1) / CHAPTERS.length) * 100}%`;
    label.textContent = `${idx + 1} / ${CHAPTERS.length}`;
  }
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

function renderChapter(id: string) {
  const chapter = findChapter(id) ?? CHAPTERS[0];
  const raw = CONTENT[chapter.id] ?? "# 内容缺失";
  const article = document.getElementById("article")!;
  article.innerHTML = renderMarkdown(raw);
  bindCopyButtons(article);

  // 在文章末尾「下一节」之前挂载可视化组件
  if (chapter.viz) {
    const vizHost = document.createElement("div");
    vizHost.className = "viz-host";
    article.appendChild(vizHost);
    mountViz(chapter.viz, vizHost);
  }

  setActive(chapter.id);
  renderPager(chapter);
  window.scrollTo({ top: 0, behavior: "instant" as ScrollBehavior });
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
