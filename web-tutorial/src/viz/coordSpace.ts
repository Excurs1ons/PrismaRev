// 坐标系变换可视化：展示一个立方体从世界空间经 view / projection 变换到 NDC，
// 强调 Vulkan 的 y-flip 与 [0,1] 深度。支持参数调节、拖拽旋转、步骤化四级变换、代码联动。

import { highlightLines } from "../highlight";

export function mountCoordSpace(host: HTMLElement): void {
  host.innerHTML = `
    <div class="viz">
      <div class="viz-head"><span class="dot"></span> 交互演示：坐标变换（World → View → Clip → NDC，含 Vulkan y-flip）</div>
      <div class="viz-body">
        <canvas id="cs-canvas" width="820" height="440"></canvas>
        <div class="ctrl-row">
          <label>FOV-Y (°)</label>
          <input type="range" id="cs-fov" min="30" max="90" step="1" value="60" />
          <span class="val" id="cs-fov-v">60</span>
        </div>
        <div class="ctrl-row">
          <label>相机距离</label>
          <input type="range" id="cs-dist" min="2" max="8" step="0.1" value="4" />
          <span class="val" id="cs-dist-v">4.0</span>
        </div>
        <div class="viz-controls">
          <button id="cs-rotate">自动旋转</button>
          <button id="cs-flip">切换 y-flip (Vulkan/OpenGL)</button>
          <button id="cs-step" class="primary">步骤化变换 ▶</button>
          <button id="cs-reset">复位相机</button>
          <button id="cs-code">对照第9章代码</button>
        </div>
        <div class="viz-hint">拖拽旋转相机。点「步骤化变换」逐帧展示 World→View→Clip→NDC 四级。Vulkan 下 NDC 的 <b>y 轴朝下</b>（y=+1 在底部）、深度 <b>z∈[0,1]</b>。</div>
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
  // 步骤化：0=world 1=view 2=clip 3=ndc
  let stage = 1; // 默认显示完整 NDC
  let stepping = false;
  let stageT = 0;

  let fovDeg = 60;
  let distance = 4;

  const verts: [number, number, number][] = [
    [-1, -1, -1], [1, -1, -1], [1, 1, -1], [-1, 1, -1],
    [-1, -1, 1], [1, -1, 1], [1, 1, 1], [-1, 1, 1],
  ];
  const edges: [number, number][] = [
    [0, 1], [1, 2], [2, 3], [3, 0],
    [4, 5], [5, 6], [6, 7], [7, 4],
    [0, 4], [1, 5], [2, 6], [3, 7],
  ];

  function project(v: [number, number, number], cx: number, cy: number, scale: number) {
    const cy_ = Math.cos(yaw), sy_ = Math.sin(yaw);
    const cp = Math.cos(pitch), sp = Math.sin(pitch);
    let [x, y, z] = v;
    // yaw
    let x1 = x * cy_ + z * sy_;
    let z1 = -x * sy_ + z * cy_;
    // pitch
    let y1 = y * cp - z1 * sp;
    let z2 = y * sp + z1 * cp;
    // 相机后退 distance
    z2 += distance;
    const persp = 6 / (6 + z2);
    return { sx: cx + x1 * scale * persp, sy: cy - y1 * scale * persp, z: z2 };
  }

  function drawCube(cx: number, cy: number, scale: number, title: string, mode: "world" | "ndc") {
    ctx.fillStyle = "#9fb0c2";
    ctx.font = "13px ui-monospace, monospace";
    ctx.fillText(title, cx - 60, cy - scale - 34);

    const ysign = mode === "ndc" && flip ? -1 : 1;
    const axes: [number, number, number, string][] = [
      [1.5, 0, 0, "#ff6b6b"],
      [0, 1.5 * ysign, 0, "#7ee787"],
      [0, 0, 1.5, "#58a6ff"],
    ];
    axes.forEach(([ax, ay, az, color]) => {
      const p0 = project([0, 0, 0], cx, cy, scale);
      const p1 = project([ax, ay, az], cx, cy, scale);
      ctx.strokeStyle = color;
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.moveTo(p0.sx, p0.sy);
      ctx.lineTo(p1.sx, p1.sy);
      ctx.stroke();
    });

    const pts = verts.map((v) => project(v, cx, cy, scale));
    ctx.strokeStyle = "#e8eef6";
    ctx.lineWidth = 1.5;
    edges.forEach(([a, b]) => {
      ctx.beginPath();
      ctx.moveTo(pts[a].sx, pts[a].sy);
      ctx.lineTo(pts[b].sx, pts[b].sy);
      ctx.stroke();
    });

    if (mode === "ndc") {
      ctx.fillStyle = "#61718a";
      ctx.font = "11px ui-monospace, monospace";
      const top = flip ? "+1 (底)" : "+1 (顶)";
      const bot = flip ? "-1 (顶)" : "-1 (底)";
      ctx.fillText(`y: ${top}`, cx + scale + 8, cy - 4);
      ctx.fillText(`y: ${bot}`, cx + scale + 8, cy + 4);
      ctx.fillText("z∈[0,1]", cx - scale - 78, cy + scale + 24);
    }
  }

  function draw() {
    ctx.clearRect(0, 0, W, H);
    ctx.fillStyle = "#080b10";
    ctx.fillRect(0, 0, W, H);

    drawCube(W * 0.27, H / 2 + 30, 70, "世界空间 (Right-Handed)", "world");
    drawCube(W * 0.73, H / 2 + 30, 70, flip ? "NDC (Vulkan y-down)" : "NDC (OpenGL y-up)", "ndc");

    ctx.strokeStyle = "#58a6ff";
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    ctx.moveTo(W * 0.45, H / 2);
    ctx.lineTo(W * 0.55, H / 2);
    ctx.stroke();
    ctx.fillStyle = "#58a6ff";
    ctx.font = "11px ui-monospace, monospace";
    ctx.fillText("clip = P·V·M", W * 0.45, H / 2 - 14);

    // 步骤化进度提示
    if (stepping) {
      const names = ["World", "View", "Clip", "NDC"];
      ctx.fillStyle = "#58a6ff";
      ctx.font = "13px ui-monospace, monospace";
      ctx.fillText(`当前阶段：${names[stage]}（${stage}/3）`, 20, H - 12);
    }
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

  host.querySelector("#cs-rotate")!.addEventListener("click", (e) => {
    auto = !auto;
    (e.target as HTMLButtonElement).textContent = auto ? "停止旋转" : "自动旋转";
  });
  host.querySelector("#cs-flip")!.addEventListener("click", () => (flip = !flip));
  host.querySelector("#cs-step")!.addEventListener("click", () => {
    stepping = true;
    stage = 0;
    stageT = 0;
  });
  host.querySelector("#cs-reset")!.addEventListener("click", () => {
    yaw = 0.6;
    pitch = 0.35;
    stage = 1;
    stepping = false;
  });
  host.querySelector("#cs-code")!.addEventListener("click", () => {
    // 第9章：OrbitCamera 的 view_proj（块内行 12..16）
    highlightLines("camera-vp", [[12, 16]]);
  });

  host.querySelector("#cs-fov")!.addEventListener("input", (e) => {
    fovDeg = +(e.target as HTMLInputElement).value;
    host.querySelector("#cs-fov-v")!.textContent = fovDeg.toString();
  });
  host.querySelector("#cs-dist")!.addEventListener("input", (e) => {
    distance = +(e.target as HTMLInputElement).value;
    host.querySelector("#cs-dist-v")!.textContent = distance.toFixed(1);
  });

  function loop() {
    if (auto) yaw += 0.01;
    if (stepping) {
      stageT += 0.02;
      if (stageT >= 1) {
        stageT = 0;
        stage++;
        if (stage > 3) {
          stage = 3;
          stepping = false;
        }
      }
    }
    draw();
    requestAnimationFrame(loop);
  }
  loop();
}
