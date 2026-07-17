// ECS 数据流可视化：展示 Entity 持有 Component，System 查询（Query）组件切片并写入结果。

export function mountEcsFlow(host: HTMLElement): void {
  host.innerHTML = `
    <div class="viz">
      <div class="viz-head"><span class="dot"></span> 交互演示：ECS 数据流（Entity → Component → System Query）</div>
      <div class="viz-body">
        <canvas id="ecs-canvas" width="820" height="380"></canvas>
        <div class="viz-controls">
          <button data-h="entity">高亮 Entity</button>
          <button data-h="component">高亮 Component</button>
          <button data-h="system">高亮 System / Query</button>
          <button data-h="none">复位</button>
        </div>
        <div class="viz-hint">点击按钮，观察数据如何以「整数句柄 + 类型化切片」的方式在 ECS 中流动——这正是 Rust 所有权下数据导向（data-oriented）的取舍。</div>
      </div>
    </div>`;

  const canvas = host.querySelector<HTMLCanvasElement>("#ecs-canvas")!;
  const ctx = canvas.getContext("2d")!;
  const W = canvas.width;
  const H = canvas.height;

  let highlight: "entity" | "component" | "system" | "none" = "none";

  const entities = [
    { id: 0, comps: ["Transform", "Mesh", "Material"] },
    { id: 1, comps: ["Transform", "Mesh", "Material"] },
    { id: 2, comps: ["Transform", "Camera"] },
    { id: 3, comps: ["Transform", "Mesh"] },
  ];

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
    ctx.lineTo(
      x2 - 8 * Math.cos(ang - 0.4),
      y2 - 8 * Math.sin(ang - 0.4)
    );
    ctx.lineTo(
      x2 - 8 * Math.cos(ang + 0.4),
      y2 - 8 * Math.sin(ang + 0.4)
    );
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
    ctx.fillStyle = "#e6edf3";
    ctx.font = "bold 14px ui-monospace, monospace";
    ctx.textAlign = "center";
    ctx.fillText(label, x + w / 2, y + (sub ? h / 2 - 4 : h / 2 + 5));
    if (sub) {
      ctx.fillStyle = "#9aa7b4";
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
    ctx.fillStyle = "#0a0d12";
    ctx.fillRect(0, 0, W, H);

    const dim = (k: string) => (highlight === "none" || highlight === k ? 1 : 0.28);

    // 中央 World
    const worldColor = `rgba(95,179,255,${dim("entity")})`;
    box(W / 2 - 70, 30, 140, 46, "#11161d", worldColor, "World", "稀疏组件池");

    // Entity 列
    const ex = 120;
    entities.forEach((e, i) => {
      const ey = 130 + i * 56;
      const col = `rgba(126,231,135,${dim("entity")})`;
      box(ex, ey, 130, 44, "#11161d", col, `Entity #${e.id}`);
      arrow(W / 2 - 70, 53, ex + 130, ey + 22, `rgba(126,231,135,${dim("entity")})`);
    });

    // Component 列（右侧）
    const comps = ["Transform", "Mesh", "Material", "Camera"];
    const cx = 560;
    comps.forEach((c, i) => {
      const cy = 130 + i * 56;
      const col = `rgba(255,157,111,${dim("component")})`;
      box(cx, cy, 130, 44, "#11161d", col, c);
    });

    // 连接 Entity -> Component（示意）
    entities.forEach((e, i) => {
      const ey = 130 + i * 56;
      e.comps.forEach((c) => {
        const ci = comps.indexOf(c);
        const cy = 130 + ci * 56;
        arrow(
          ex + 130,
          ey + 22,
          cx,
          cy + 22,
          `rgba(255,157,111,${dim("component") * 0.6})`
        );
      });
    });

    // System（底部）
    const sysY = 360;
    const sysCol = `rgba(240,198,116,${dim("system")})`;
    box(W / 2 - 110, sysY - 30, 220, 40, "#11161d", sysCol, "RenderSystem", "Query<Transform,Mesh,Material>");
    // System -> World query 箭头
    arrow(W / 2, sysY - 30, W / 2, 76, `rgba(240,198,116,${dim("system")})`);
    // System 写入到 GPU/帧
    arrow(W / 2 + 110, sysY - 10, W - 30, sysY - 10, `rgba(240,198,116,${dim("system")})`);
    ctx.fillStyle = `rgba(240,198,116,${dim("system")})`;
    ctx.font = "11px ui-monospace, monospace";
    ctx.fillText("→ 录制命令缓冲 / 绘制", W - 200, sysY - 16);
  }

  host.querySelectorAll<HTMLButtonElement>("button[data-h]").forEach((b) => {
    b.addEventListener("click", () => {
      highlight = (b.getAttribute("data-h") as any) || "none";
      draw();
    });
  });

  draw();
}
