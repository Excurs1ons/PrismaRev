// 部署流程可视化：展示从同一份 Rust 源码如何分别产出桌面 exe 与 Android APK，
// 强调 winit 后端切换、NDK 链接、android-activity 入口。

export function mountDeployFlow(host: HTMLElement): void {
  host.innerHTML = `
    <div class="viz">
      <div class="viz-head"><span class="dot"></span> 交互演示：同一份引擎 → 桌面 exe / Android APK 的两条构建链路</div>
      <div class="viz-body">
        <canvas id="df-canvas" width="820" height="420"></canvas>
        <div class="viz-controls">
          <button data-p="desktop">桌面链路</button>
          <button data-p="android">Android 链路</button>
          <button data-p="both">两条链路</button>
        </div>
        <div class="viz-hint">点击查看每条链路的工具链差异：桌面用系统 Vulkan Loader，Android 走 NDK clang + android-activity。</div>
      </div>
    </div>`;

  const canvas = host.querySelector<HTMLCanvasElement>("#df-canvas")!;
  const ctx = canvas.getContext("2d")!;
  const W = canvas.width;
  const H = canvas.height;

  let path: "desktop" | "android" | "both" = "both";

  const desktopSteps = [
    "cargo run",
    "prism-engine (winit 桌面后端)",
    "Vulkan Loader (vulkan-1.dll)",
    "prismarev.exe",
  ];
  const androidSteps = [
    "cargo build --target aarch64-linux-android",
    "prism-android (winit android-game-activity)",
    "NDK clang 链接 + android-activity",
    "Gradle → prismarev.apk",
  ];

  function node(x: number, y: number, w: number, h: number, label: string, active: boolean, color: string) {
    ctx.globalAlpha = active ? 1 : 0.25;
    ctx.fillStyle = "#11161d";
    ctx.strokeStyle = color;
    ctx.lineWidth = 1.5;
    roundRect(x, y, w, h, 8);
    ctx.fill();
    ctx.stroke();
    ctx.fillStyle = active ? "#e6edf3" : "#6b7785";
    ctx.font = "12px ui-monospace, monospace";
    ctx.textAlign = "center";
    const lines = label.split("\n");
    lines.forEach((ln, i) => {
      ctx.fillText(ln, x + w / 2, y + h / 2 - (lines.length - 1) * 8 + i * 16 + 4);
    });
    ctx.textAlign = "left";
    ctx.globalAlpha = 1;
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

  function flow(steps: string[], x: number, color: string, active: boolean) {
    const y0 = 70;
    const gap = 80;
    steps.forEach((s, i) => {
      const label = s.replace(/ \(.*\)/, (m) => "\n" + m.slice(1, -1));
      node(x, y0 + i * gap, 280, 54, label, active, color);
      if (i < steps.length - 1 && active) {
        ctx.strokeStyle = color;
        ctx.lineWidth = 2;
        ctx.beginPath();
        ctx.moveTo(x + 140, y0 + i * gap + 54);
        ctx.lineTo(x + 140, y0 + (i + 1) * gap);
        ctx.stroke();
        // 箭头
        ctx.beginPath();
        ctx.moveTo(x + 140, y0 + (i + 1) * gap);
        ctx.lineTo(x + 135, y0 + (i + 1) * gap - 8);
        ctx.lineTo(x + 145, y0 + (i + 1) * gap - 8);
        ctx.closePath();
        ctx.fillStyle = color;
        ctx.fill();
      }
    });
  }

  function draw() {
    ctx.clearRect(0, 0, W, H);
    ctx.fillStyle = "#0a0d12";
    ctx.fillRect(0, 0, W, H);

    // 公共源码根
    node(W / 2 - 120, 10, 240, 44, "同一份 Rust 源码\n(prism-ecs / prism-render / prism-engine)", true, "#7ee787");
    ctx.strokeStyle = "#7ee787";
    ctx.lineWidth = 2;
    [W / 2 - 120, W / 2 + 120].forEach((x) => {
      ctx.beginPath();
      ctx.moveTo(x, 54);
      ctx.lineTo(x, 70);
      ctx.stroke();
    });

    ctx.fillStyle = "#9aa7b4";
    ctx.font = "bold 13px ui-monospace, monospace";
    ctx.fillText("桌面", 175, 64);
    ctx.fillText("Android", 565, 64);

    const dActive = path === "desktop" || path === "both";
    const aActive = path === "android" || path === "both";
    flow(desktopSteps, 30, "#5fb3ff", dActive);
    flow(androidSteps, 510, "#ff9d6f", aActive);
  }

  host.querySelectorAll<HTMLButtonElement>("button[data-p]").forEach((b) => {
    b.addEventListener("click", () => {
      path = (b.getAttribute("data-p") as any) || "both";
      draw();
    });
  });

  draw();
}
