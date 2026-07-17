// 坐标系变换可视化：展示一个 3D 立方体在世界空间被相机观察，经 view/projection
// 变换后得到裁剪空间，再经透视除法得到 NDC，并强调 Vulkan 的 y-flip 与 [0,1] 深度。

export function mountCoordSpace(host: HTMLElement): void {
  host.innerHTML = `
    <div class="viz">
      <div class="viz-head"><span class="dot"></span> 交互演示：坐标变换（World → View → Clip → NDC，含 Vulkan y-flip）</div>
      <div class="viz-body">
        <canvas id="cs-canvas" width="820" height="420"></canvas>
        <div class="viz-controls">
          <button id="cs-rotate">自动旋转</button>
          <button id="cs-flip">切换 y-flip 对比 (Vulkan vs OpenGL)</button>
          <button id="cs-reset">复位相机</button>
        </div>
        <div class="viz-hint">拖拽画面旋转相机；观察右侧 NDC 立方体：Vulkan 下 y 轴向下（+1 在底部），深度 z ∈ [0,1]。</div>
      </div>
    </div>`;

  const canvas = host.querySelector<HTMLCanvasElement>("#cs-canvas")!;
  const ctx = canvas.getContext("2d")!;
  const W = canvas.width;
  const H = canvas.height;

  let yaw = 0.6;
  let pitch = 0.35;
  let auto = false;
  let flip = true; // true=Vulkan, false=OpenGL
  let dragging = false;
  let lastX = 0;
  let lastY = 0;

  // 立方体顶点 [-1,1]
  const verts: [number, number, number][] = [
    [-1, -1, -1],
    [1, -1, -1],
    [1, 1, -1],
    [-1, 1, -1],
    [-1, -1, 1],
    [1, -1, 1],
    [1, 1, 1],
    [-1, 1, 1],
  ];
  const edges: [number, number][] = [
    [0, 1], [1, 2], [2, 3], [3, 0],
    [4, 5], [5, 6], [6, 7], [7, 4],
    [0, 4], [1, 5], [2, 6], [3, 7],
  ];

  function project(v: [number, number, number], cx: number, cy: number, scale: number, depth: number) {
    // 绕 Y 轴 yaw，绕 X 轴 pitch
    const cy_ = Math.cos(yaw), sy_ = Math.sin(yaw);
    const cp = Math.cos(pitch), sp = Math.sin(pitch);
    let [x, y, z] = v;
    // yaw
    let x1 = x * cy_ + z * sy_;
    let z1 = -x * sy_ + z * cy_;
    // pitch
    let y1 = y * cp - z1 * sp;
    let z2 = y * sp + z1 * cp;
    const persp = depth / (depth + z2 + 4);
    return { sx: cx + x1 * scale * persp, sy: cy - y1 * scale * persp, z: z2 };
  }

  function drawCube(cx: number, cy: number, scale: number, title: string, mode: "world" | "ndc") {
    ctx.fillStyle = "#9aa7b4";
    ctx.font = "13px ui-monospace, monospace";
    ctx.fillText(title, cx - 60, cy - scale - 30);

    // 轴
    const ysign = mode === "ndc" && flip ? -1 : 1; // Vulkan ndc y 向下
    const axes: [number, number, number, string][] = [
      [1.4, 0, 0, "#ff6b6b"], // X
      [0, 1.4 * ysign, 0, "#7ee787"], // Y
      [0, 0, 1.4, "#5fb3ff"], // Z
    ];
    axes.forEach(([ax, ay, az, color]) => {
      const p0 = project([0, 0, 0], cx, cy, scale, 6);
      const p1 = project([ax, ay, az], cx, cy, scale, 6);
      ctx.strokeStyle = color;
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.moveTo(p0.sx, p0.sy);
      ctx.lineTo(p1.sx, p1.sy);
      ctx.stroke();
    });

    const pts = verts.map((v) => project(v, cx, cy, scale, 6));
    ctx.strokeStyle = "#e6edf3";
    ctx.lineWidth = 1.5;
    edges.forEach(([a, b]) => {
      ctx.beginPath();
      ctx.moveTo(pts[a].sx, pts[a].sy);
      ctx.lineTo(pts[b].sx, pts[b].sy);
      ctx.stroke();
    });

    if (mode === "ndc") {
      // 标注 NDC 范围
      ctx.fillStyle = "#6b7785";
      ctx.font = "11px ui-monospace, monospace";
      const top = flip ? "+1 (底)" : "+1 (顶)";
      const bot = flip ? "-1 (顶)" : "-1 (底)";
      ctx.fillText(`y: ${top}`, cx + scale + 6, cy - 4);
      ctx.fillText(`y: ${bot}`, cx + scale + 6, cy + 4);
      ctx.fillText("z ∈ [0,1]", cx - scale - 70, cy + scale + 20);
    }
  }

  function draw() {
    ctx.clearRect(0, 0, W, H);
    ctx.fillStyle = "#0a0d12";
    ctx.fillRect(0, 0, W, H);

    // 左：世界空间（右手系）
    drawCube(W * 0.27, H / 2 + 30, 70, "世界空间 (Right-Handed)", "world");
    // 右：NDC（受 y-flip 影响）
    drawCube(W * 0.73, H / 2 + 30, 70, flip ? "NDC (Vulkan y-down)" : "NDC (OpenGL y-up)", "ndc");

    // 中间箭头
    ctx.strokeStyle = "#5fb3ff";
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(W * 0.45, H / 2);
    ctx.lineTo(W * 0.55, H / 2);
    ctx.stroke();
    ctx.fillStyle = "#5fb3ff";
    ctx.font = "11px ui-monospace, monospace";
    ctx.fillText("clip = P·V·M", W * 0.45, H / 2 - 12);
  }

  canvas.addEventListener("mousedown", (e) => {
    dragging = true;
    lastX = e.offsetX;
    lastY = e.offsetY;
  });
  window.addEventListener("mouseup", () => (dragging = false));
  canvas.addEventListener("mousemove", (e) => {
    if (!dragging) return;
    yaw += (e.offsetX - lastX) * 0.01;
    pitch += (e.offsetY - lastY) * 0.01;
    pitch = Math.max(-1.4, Math.min(1.4, pitch));
    lastX = e.offsetX;
    lastY = e.offsetY;
  });

  host.querySelector<HTMLButtonElement>("#cs-rotate")!.addEventListener("click", (e) => {
    auto = !auto;
    (e.target as HTMLButtonElement).textContent = auto ? "停止旋转" : "自动旋转";
  });
  host.querySelector<HTMLButtonElement>("#cs-flip")!.addEventListener("click", () => {
    flip = !flip;
  });
  host.querySelector<HTMLButtonElement>("#cs-reset")!.addEventListener("click", () => {
    yaw = 0.6;
    pitch = 0.35;
  });

  function loop() {
    if (auto) yaw += 0.01;
    draw();
    requestAnimationFrame(loop);
  }
  loop();
}
