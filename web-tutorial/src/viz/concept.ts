// 关键概念图：可交互的静态/半静态示意图，配合章节理解架构与数据流。
import type { VizKey } from "../chapters";

function wrap(host: HTMLElement, title: string, inner: string): void {
  host.innerHTML = `
    <div class="viz">
      <div class="viz-head"><span class="dot"></span> ${title}</div>
      <div class="viz-body">${inner}</div>
    </div>`;
}

function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

// 内存布局：SlotMap 句柄 → 稀疏池 → 组件数据
export function mountMemory(host: HTMLElement): void {
  wrap(
    host,
    "交互演示：组件内存布局（SlotMap 句柄 → 类型化稀疏池）",
    `<canvas id="mem-canvas" width="820" height="360"></canvas>
     <div class="viz-controls"><button id="mem-q">高亮 Transform 池</button><button id="mem-r">复位</button></div>
     <div class="viz-hint">每个组件类型一个稀疏池（keyed by TypeId）。实体只是整数句柄；系统 query 时按 TypeId 取池、按 id 取切片。点击按钮高亮某一池。</div>`
  );
  const canvas = host.querySelector<HTMLCanvasElement>("#mem-canvas")!;
  const ctx = canvas.getContext("2d")!;
  const W = canvas.width, H = canvas.height;
  let hi: number | null = null;

  const pools = [
    { name: "Transform", color: "#ff9d6f", vals: ["e0", "e1", "e2", "e3"] },
    { name: "Mesh", color: "#58a6ff", vals: ["e0", "e1", "e3"] },
    { name: "Material", color: "#7ee787", vals: ["e0", "e1"] },
    { name: "Camera", color: "#f0c674", vals: ["e2"] },
  ];

  function draw() {
    ctx.clearRect(0, 0, W, H);
    ctx.fillStyle = "#080b10";
    ctx.fillRect(0, 0, W, H);
    // 实体句柄列
    ctx.fillStyle = "#9fb0c2";
    ctx.font = "12px ui-monospace, monospace";
    ctx.fillText("Entity 句柄", 20, 24);
    for (let i = 0; i < 4; i++) {
      roundRect(ctx, 20, 40 + i * 40, 70, 30, 6);
      ctx.fillStyle = "#0f141c"; ctx.fill();
      ctx.strokeStyle = "#2a3543"; ctx.stroke();
      ctx.fillStyle = "#e8eef6"; ctx.textAlign = "center";
      ctx.fillText(`#${i}`, 55, 40 + i * 40 + 20);
      ctx.textAlign = "left";
    }
    // 池
    pools.forEach((p, pi) => {
      const y = 40;
      const px = 150 + pi * 165;
      ctx.fillStyle = hi === pi ? p.color : "#9fb0c2";
      ctx.font = "bold 12px ui-monospace, monospace";
      ctx.fillText(`${p.name} 池`, px, 30);
      p.vals.forEach((v, vi) => {
        roundRect(ctx, px, y + vi * 34, 130, 28, 6);
        ctx.fillStyle = hi === pi ? "rgba(255,255,255,0.06)" : "#0f141c";
        ctx.fill();
        ctx.strokeStyle = hi === pi ? p.color : "#2a3543";
        ctx.stroke();
        ctx.fillStyle = "#e8eef6"; ctx.textAlign = "center";
        ctx.fillText(v, px + 65, y + vi * 34 + 18);
        ctx.textAlign = "left";
        // 连线到实体
        if (hi === pi || hi === null) {
          const ei = parseInt(v.slice(1));
          ctx.strokeStyle = hi === pi ? p.color : "#2a3543";
          ctx.lineWidth = hi === pi ? 1.5 : 1;
          ctx.beginPath();
          ctx.moveTo(px, y + vi * 34 + 14);
          ctx.lineTo(90, 40 + ei * 40 + 15);
          ctx.stroke();
        }
      });
    });
  }
  host.querySelector("#mem-q")!.addEventListener("click", () => { hi = 0; draw(); });
  host.querySelector("#mem-r")!.addEventListener("click", () => { hi = null; draw(); });
  draw();
}

// 渲染图：pass 依赖 DAG（可点击看说明）
export function mountRenderGraph(host: HTMLElement): void {
  wrap(
    host,
    "交互演示：Render Graph（pass 依赖 DAG）",
    `<canvas id="rg-canvas" width="820" height="320"></canvas>
     <div class="viz-hint">点击节点查看该 pass 职责。箭头表示依赖：上游 pass 的输出是下游的输入。引擎用 render_graph.rs 描述并拓扑排序执行。</div>`
  );
  const canvas = host.querySelector<HTMLCanvasElement>("#rg-canvas")!;
  const ctx = canvas.getContext("2d")!;
  const W = canvas.width, H = canvas.height;
  const nodes = [
    { id: "shadow", label: "Shadow", x: 60, y: 60, desc: "阴影贴图 pass：从光源视角渲染深度" },
    { id: "gbuffer", label: "GBuffer", x: 320, y: 60, desc: "延迟渲染：输出位置/法线/反照率" },
    { id: "light", label: "Lighting", x: 320, y: 180, desc: "PBR + IBL 光照合成" },
    { id: "post", label: "Post", x: 580, y: 180, desc: "后处理：tone mapping / bloom" },
  ];
  const edges = [["shadow", "light"], ["gbuffer", "light"], ["light", "post"]];
  let sel: string | null = null;

  function draw() {
    ctx.clearRect(0, 0, W, H);
    ctx.fillStyle = "#080b10"; ctx.fillRect(0, 0, W, H);
    edges.forEach(([a, b]) => {
      const na = nodes.find((n) => n.id === a)!;
      const nb = nodes.find((n) => n.id === b)!;
      ctx.strokeStyle = "#2a3543"; ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.moveTo(na.x + 70, na.y + 25);
      ctx.lineTo(nb.x, nb.y + 25);
      ctx.stroke();
      ctx.fillStyle = "#2a3543";
      ctx.beginPath();
      ctx.moveTo(nb.x, nb.y + 25);
      ctx.lineTo(nb.x - 8, nb.y + 19);
      ctx.lineTo(nb.x - 8, nb.y + 31);
      ctx.closePath(); ctx.fill();
    });
    nodes.forEach((n) => {
      const active = sel === n.id;
      roundRect(ctx, n.x, n.y, 140, 50, 8);
      ctx.fillStyle = active ? "rgba(88,166,255,0.14)" : "#0f141c";
      ctx.fill();
      ctx.strokeStyle = active ? "#58a6ff" : "#2a3543";
      ctx.lineWidth = active ? 2 : 1.5; ctx.stroke();
      ctx.fillStyle = "#e8eef6"; ctx.textAlign = "center";
      ctx.font = "bold 14px ui-monospace, monospace";
      ctx.fillText(n.label, n.x + 70, n.y + 30);
      ctx.textAlign = "left";
    });
    if (sel) {
      const n = nodes.find((x) => x.id === sel)!;
      ctx.fillStyle = "#9fb0c2"; ctx.font = "13px ui-monospace, monospace";
      ctx.fillText(`▸ ${n.desc}`, 20, H - 16);
    }
  }
  canvas.addEventListener("click", (e) => {
    const r = canvas.getBoundingClientRect();
    const mx = (e.clientX - r.left) * (W / r.width);
    const my = (e.clientY - r.top) * (H / r.height);
    for (const n of nodes) {
      if (mx >= n.x && mx <= n.x + 140 && my >= n.y && my <= n.y + 50) {
        sel = sel === n.id ? null : n.id;
        draw();
        return;
      }
    }
  });
  draw();
}

// Pipeline 状态流水线：固定功能阶段
export function mountPipeline(host: HTMLElement): void {
  wrap(
    host,
    "交互演示：图形管线状态流水线（固定功能阶段）",
    `<canvas id="pl-canvas" width="820" height="240"></canvas>
     <div class="viz-hint">点击任一阶段查看其职责。Vulkan 把这些状态在创建管线时固化，运行时切换不同管线而非改参数。</div>`
  );
  const canvas = host.querySelector<HTMLCanvasElement>("#pl-canvas")!;
  const ctx = canvas.getContext("2d")!;
  const W = canvas.width, H = canvas.height;
  const stages = [
    { t: "IA", d: "输入装配：顶点/索引如何组成图元" },
    { t: "VS", d: "顶点着色：clip = P·V·M" },
    { t: "RS", d: "光栅化：图元 → 片元" },
    { t: "FS", d: "片元着色：PBR/Blind-Phong 上色" },
    { t: "CB", d: "颜色混合：写入附件" },
  ];
  let sel = -1;
  function draw() {
    ctx.clearRect(0, 0, W, H);
    ctx.fillStyle = "#080b10"; ctx.fillRect(0, 0, W, H);
    const n = stages.length;
    const w = (W - 60) / n;
    stages.forEach((s, i) => {
      const x = 30 + i * w;
      const active = sel === i;
      roundRect(ctx, x + 6, 50, w - 12, 70, 8);
      ctx.fillStyle = active ? "rgba(126,231,135,0.14)" : "#0f141c";
      ctx.fill();
      ctx.strokeStyle = active ? "#7ee787" : "#2a3543";
      ctx.lineWidth = active ? 2 : 1.5; ctx.stroke();
      ctx.fillStyle = "#e8eef6"; ctx.textAlign = "center";
      ctx.font = "bold 15px ui-monospace, monospace";
      ctx.fillText(s.t, x + w / 2, 90);
      if (i < n - 1) {
        ctx.strokeStyle = "#2a3543"; ctx.lineWidth = 2;
        ctx.beginPath(); ctx.moveTo(x + w, 85); ctx.lineTo(x + w + 6, 85); ctx.stroke();
      }
    });
    if (sel >= 0) {
      ctx.fillStyle = "#9fb0c2"; ctx.font = "13px ui-monospace, monospace";
      ctx.fillText(`▸ ${stages[sel].t}：${stages[sel].d}`, 30, H - 20);
    }
  }
  canvas.addEventListener("click", (e) => {
    const r = canvas.getBoundingClientRect();
    const mx = (e.clientX - r.left) * (W / r.width);
    const n = stages.length;
    const w = (W - 60) / n;
    for (let i = 0; i < n; i++) {
      const x = 30 + i * w;
      if (mx >= x + 6 && mx <= x + w - 6) { sel = i; draw(); return; }
    }
  });
  draw();
}

// 坐标系链：World → View → Clip → NDC 四级框
export function mountCoordChain(host: HTMLElement): void {
  wrap(
    host,
    "交互演示：坐标变换链条（World → View → Clip → NDC）",
    `<canvas id="cc-canvas" width="820" height="200"></canvas>
     <div class="viz-hint">四个空间逐级映射。引擎严格遵守：世界/视图右手系 → clip 做 Vulkan y-flip → NDC 的 y 向下、z∈[0,1]。与上方交互立方体联动理解。</div>`
  );
  const canvas = host.querySelector<HTMLCanvasElement>("#cc-canvas")!;
  const ctx = canvas.getContext("2d")!;
  const W = canvas.width, H = canvas.height;
  const boxes = [
    { t: "World", s: "右手系 +X右/+Y上/+Z朝观察者" },
    { t: "View", s: "相机系：看向 -Z" },
    { t: "Clip", s: "proj*view*model（y-flip）" },
    { t: "NDC", s: "x,y∈[-1,1] y向下; z∈[0,1]" },
  ];
  function draw() {
    ctx.clearRect(0, 0, W, H);
    ctx.fillStyle = "#080b10"; ctx.fillRect(0, 0, W, H);
    const n = boxes.length;
    const w = (W - 80) / n;
    const colors = ["#7ee787", "#58a6ff", "#f0c674", "#ff9d6f"];
    boxes.forEach((b, i) => {
      const x = 20 + i * (w + 20);
      roundRect(ctx, x, 40, w, 90, 8);
      ctx.fillStyle = "#0f141c"; ctx.fill();
      ctx.strokeStyle = colors[i]; ctx.lineWidth = 1.5; ctx.stroke();
      ctx.fillStyle = colors[i]; ctx.textAlign = "center";
      ctx.font = "bold 15px ui-monospace, monospace";
      ctx.fillText(b.t, x + w / 2, 72);
      ctx.fillStyle = "#9fb0c2"; ctx.font = "10px ui-monospace, monospace";
      wrapText(ctx, b.s, x + w / 2, 92, w - 12);
      ctx.textAlign = "left";
      if (i < n - 1) {
        ctx.strokeStyle = colors[i]; ctx.lineWidth = 2;
        ctx.beginPath(); ctx.moveTo(x + w, 85); ctx.lineTo(x + w + 20, 85); ctx.stroke();
        ctx.beginPath(); ctx.moveTo(x + w + 20, 85);
        ctx.lineTo(x + w + 12, 80); ctx.lineTo(x + w + 12, 90); ctx.closePath();
        ctx.fillStyle = colors[i]; ctx.fill();
      }
    });
  }
  function wrapText(c: CanvasRenderingContext2D, text: string, cx: number, y: number, maxw: number) {
    const words = text.split(" ");
    let line = "";
    const lines: string[] = [];
    for (const wd of words) {
      if (c.measureText(line + wd).width > maxw && line) { lines.push(line); line = wd; }
      else line += (line ? " " : "") + wd;
    }
    if (line) lines.push(line);
    lines.forEach((l, i) => c.fillText(l, cx, y + i * 13));
  }
  draw();
}

export function mountConcept(key: VizKey, host: HTMLElement): void {
  switch (key) {
    case "memory": return mountMemory(host);
    case "rendergraph": return mountRenderGraph(host);
    case "pipeline": return mountPipeline(host);
    case "coordchain": return mountCoordChain(host);
    default: return;
  }
}
