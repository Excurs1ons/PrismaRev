// ECS 数据流可视化：展示 Entity 持有 Component，System 通过 Query 取出组件切片。
// 支持按组件类型高亮、单步演示 spawn→insert→query、代码联动。

import { highlightLines } from "../highlight";

export function mountEcsFlow(host: HTMLElement): void {
  host.innerHTML = `
    <div class="viz">
      <div class="viz-head"><span class="dot"></span> 交互演示：ECS 数据流（Entity → Component → System Query）</div>
      <div class="viz-body">
        <canvas id="ecs-canvas" width="820" height="400"></canvas>
        <div class="viz-controls">
          <button data-h="entity">高亮 Entity</button>
          <button data-h="component">高亮 Component</button>
          <button data-h="system">高亮 System / Query</button>
          <button data-h="none">复位</button>
          <button id="ecs-step" class="primary">单步演示 ▶</button>
          <button id="ecs-code">对照第8章代码</button>
        </div>
        <div class="viz-hint">点击按钮观察数据如何以「整数句柄 + 类型化切片」流动。点「单步演示」可看 spawn 实体 → insert 组件 → system query 三步走。当前拥有 <b id="ecs-count">Transform</b> 的实体数会实时变化。</div>
      </div>
    </div>`;

  const canvas = host.querySelector<HTMLCanvasElement>("#ecs-canvas")!;
  const ctx = canvas.getContext("2d")!;
  const W = canvas.width;
  const H = canvas.height;

  type Step = "idle" | "spawn" | "insert" | "query";
  let highlight: "entity" | "component" | "system" | "none" = "none";
  let step: Step = "idle";
  let stepT = 0;

  const entities = [
    { id: 0, comps: ["Transform", "Mesh", "Material"] },
    { id: 1, comps: ["Transform", "Mesh", "Material"] },
    { id: 2, comps: ["Transform", "Camera"] },
    { id: 3, comps: ["Transform", "Mesh"] },
  ];
  // 单步演示：第 4 个实体动态出现
  let extraEntity: { id: number; comps: string[] } | null = null;

  function countWith(comp: string): number {
    let n = entities.filter((e) => e.comps.includes(comp)).length;
    if (extraEntity && extraEntity.comps.includes(comp)) n++;
    return n;
  }

  function arrow(x1: number, y1: number, x2: number, y2: number, color: string) {
    ctx.strokeStyle = color;
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(x1, y1);
    ctx.lineTo(x2, y2);
    ctx.stroke();
    const ang = Math.atan2(y2 - y1, x2 - x1);
    ctx.beginPath();
    ctx.moveTo(x2, y2);
    ctx.lineTo(x2 - 8 * Math.cos(ang - 0.4), y2 - 8 * Math.sin(ang - 0.4));
    ctx.lineTo(x2 - 8 * Math.cos(ang + 0.4), y2 - 8 * Math.sin(ang + 0.4));
    ctx.closePath();
    ctx.fillStyle = color;
    ctx.fill();
  }

  function box(x: number, y: number, w: number, h: number, fill: string, stroke: string, label: string, sub?: string) {
    ctx.fillStyle = fill;
    ctx.strokeStyle = stroke;
    ctx.lineWidth = 1.5;
    roundRect(x, y, w, h, 8);
    ctx.fill();
    ctx.stroke();
    ctx.fillStyle = "#e8eef6";
    ctx.font = "bold 14px ui-monospace, monospace";
    ctx.textAlign = "center";
    ctx.fillText(label, x + w / 2, y + (sub ? h / 2 - 4 : h / 2 + 5));
    if (sub) {
      ctx.fillStyle = "#9fb0c2";
      ctx.font = "11px ui-monospace, monospace";
      ctx.fillText(sub, x + w / 2, y + h / 2 + 14);
    }
    ctx.textAlign = "left";
  }

  function roundRect(x: number, y: number, w: number, h: number, r: number) {
    ctx.beginPath();
    ctx.moveTo(x + r, y);
    ctx.arcTo(x + w, y, x + w, y + h, r);
    ctx.arcTo(x + w, y + h, x, y + h, r);
    ctx.arcTo(x, y + h, x, y, r);
    ctx.arcTo(x, y, x + w, y, r);
    ctx.closePath();
  }

  function draw() {
    ctx.clearRect(0, 0, W, H);
    ctx.fillStyle = "#080b10";
    ctx.fillRect(0, 0, W, H);

    const dim = (k: string) => (highlight === "none" || highlight === k ? 1 : 0.26);
    const spawnA = step === "spawn" ? 0.4 + 0.6 * (1 - stepT) : 1;

    box(W / 2 - 70, 30, 140, 46, "#0f141c", `rgba(88,166,255,${dim("entity")})`, "World", "稀疏组件池");

    const ex = 120;
    const list = extraEntity ? [...entities, extraEntity] : entities;
    list.forEach((e, i) => {
      const ey = 130 + i * 58;
      const col = `rgba(126,231,135,${dim("entity") * (e === extraEntity ? spawnA : 1)})`;
      box(ex, ey, 130, 44, "#0f141c", col, `Entity #${e.id}`);
      arrow(W / 2 - 70, 53, ex + 130, ey + 22, `rgba(126,231,135,${dim("entity")})`);
    });

    const comps = ["Transform", "Mesh", "Material", "Camera"];
    const cx = 560;
    comps.forEach((c, i) => {
      const cy = 130 + i * 58;
      const col = `rgba(255,157,111,${dim("component")})`;
      box(cx, cy, 130, 44, "#0f141c", col, c);
    });

    list.forEach((e, i) => {
      const ey = 130 + i * 58;
      e.comps.forEach((c) => {
        const ci = comps.indexOf(c);
        const cy = 130 + ci * 58;
        const active = step === "insert" && e === extraEntity;
        arrow(
          ex + 130,
          ey + 22,
          cx,
          cy + 22,
          `rgba(255,157,111,${dim("component") * (active ? 1 : 0.5)})`
        );
      });
    });

    const sysY = 378;
    const sysCol = `rgba(240,198,116,${dim("system")})`;
    box(W / 2 - 130, sysY - 30, 260, 42, "#0f141c", sysCol, "RenderSystem", "Query<Transform,Mesh,Material>");
    arrow(W / 2, sysY - 30, W / 2, 76, `rgba(240,198,116,${dim("system")})`);
    arrow(W / 2 + 130, sysY - 10, W - 30, sysY - 10, `rgba(240,198,116,${dim("system")})`);
    ctx.fillStyle = `rgba(240,198,116,${dim("system")})`;
    ctx.font = "11px ui-monospace, monospace";
    ctx.fillText("→ 录制命令缓冲 / 绘制", W - 220, sysY - 16);

    if (step !== "idle") {
      ctx.fillStyle = "#58a6ff";
      ctx.font = "13px ui-monospace, monospace";
      const msg =
        step === "spawn"
          ? "① spawn() → 分配新实体句柄 #" + (extraEntity?.id ?? 4)
          : step === "insert"
          ? "② insert() → 把组件写入该实体的类型化池"
          : "③ query() → 系统筛出「同时拥有 Transform+Mesh+Material」的实体";
      ctx.fillText(msg, 20, H - 12);
    }
  }

  function tick() {
    if (step !== "idle") {
      stepT += 0.02;
      if (stepT >= 1) {
        stepT = 0;
        if (step === "spawn") {
          extraEntity = { id: 4, comps: [] };
          step = "insert";
        } else if (step === "insert") {
          extraEntity!.comps = ["Transform", "Mesh", "Material"];
          step = "query";
        } else {
          step = "idle";
        }
      }
    }
    draw();
    requestAnimationFrame(tick);
  }

  host.querySelectorAll<HTMLButtonElement>("button[data-h]").forEach((b) => {
    b.addEventListener("click", () => {
      highlight = (b.getAttribute("data-h") as any) || "none";
      draw();
    });
  });
  host.querySelector("#ecs-step")!.addEventListener("click", () => {
    if (step === "idle") {
      extraEntity = null;
      step = "spawn";
      stepT = 0;
    }
  });
  host.querySelector("#ecs-code")!.addEventListener("click", () => {
    // 第8章：query2 代码块（块内行 1..12 为函数签名与体）
    highlightLines("ecs-query", [[1, 12]]);
  });

  host.querySelector("#ecs-count")!.textContent = `Transform (${countWith("Transform")})`;
  tick();
}
