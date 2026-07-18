// 部署流程可视化：展示同一份 Rust 源码如何分别产出桌面 exe 与 Android APK，
// 强调 winit 后端切换、NDK 链接、android-activity 入口。支持链路切换、代码联动。

import { highlightLines } from "../highlight";

export function mountDeployFlow(host: HTMLElement): void {
  host.innerHTML = `
    <div class="viz">
      <div class="viz-head"><span class="dot"></span> 交互演示：同一份引擎 → 桌面 exe / Android APK 的两条构建链路</div>
      <div class="viz-body">
        <canvas id="df-canvas" width="820" height="430"></canvas>
        <div class="viz-controls">
          <button data-p="desktop">桌面链路</button>
          <button data-p="android">Android 链路</button>
          <button data-p="both" class="primary">两条链路</button>
          <button id="df-code">对照第12章代码</button>
        </div>
        <div class="viz-hint">点击查看每条链路的工具链差异：桌面用系统 Vulkan Loader，Android 走 NDK clang + android-activity。.so 经 Gradle 打包成 APK。</div>
      </div>
    </div>`;

  const canvas = host.querySelector<HTMLCanvasElement>("#df-canvas")!;
  const ctx = canvas.getContext("2d")!;
  const W = canvas.width;
  const H = canvas.height;

  let path: "desktop" | "android" | "both" = "both";

  const desktopSteps = [
    "cargo run",
    "prism-engine\n(winit 桌面后端)",
    "Vulkan Loader\n(vulkan-1.dll)",
    "prismarev.exe\n(~3 MB)",
  ];
  const androidSteps = [
    "cargo ndk\n--target aarch64",
    "prism-android\n(winit android-game-activity)",
    "NDK clang 链接\n+ android-activity",
    "Gradle →\nprismarev.apk",
  ];

  function node(x: number, y: number, w: number, h: number, label: string, active: boolean, color: string) {
    ctx.globalAlpha = active ? 1 : 0.22;
    ctx.fillStyle = "#0f141c";
    ctx.strokeStyle = color;
    ctx.lineWidth = 1.5;
    roundRect(x, y, w, h, 8);
    ctx.fill();
    ctx.stroke();
    ctx.fillStyle = active ? "#e8eef6" : "#61718a";
    ctx.font = "12px ui-monospace, monospace";
    ctx.textAlign = "center";
    label.split("\n").forEach((ln, i, arr) => {
      ctx.fillText(ln, x + w / 2, y + h / 2 - (arr.length - 1) * 9 + i * 18 + 4);
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
    const y0 = 78;
    const gap = 84;
    steps.forEach((s, i) => {
      node(x, y0 + i * gap, 280, 56, s, active, color);
      if (i < steps.length - 1 && active) {
        ctx.strokeStyle = color;
        ctx.lineWidth = 2;
        ctx.beginPath();
        ctx.moveTo(x + 140, y0 + i * gap + 56);
        ctx.lineTo(x + 140, y0 + (i + 1) * gap);
        ctx.stroke();
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
    ctx.fillStyle = "#080b10";
    ctx.fillRect(0, 0, W, H);

    node(W / 2 - 130, 12, 260, 46, "同一份 Rust 源码\n(prism-ecs/render/asset/engine)", true, "#7ee787");
    ctx.strokeStyle = "#7ee787";
    ctx.lineWidth = 2;
    [W / 2 - 130, W / 2 + 130].forEach((x) => {
      ctx.beginPath();
      ctx.moveTo(x, 58);
      ctx.lineTo(x, 78);
      ctx.stroke();
    });

    ctx.fillStyle = "#9fb0c2";
    ctx.font = "bold 13px ui-monospace, monospace";
    ctx.fillText("桌面", 170, 72);
    ctx.fillText("Android", 555, 72);

    const dActive = path === "desktop" || path === "both";
    const aActive = path === "android" || path === "both";
    flow(desktopSteps, 30, "#58a6ff", dActive);
    flow(androidSteps, 510, "#ff9d6f", aActive);
  }

  host.querySelectorAll<HTMLButtonElement>("button[data-p]").forEach((b) => {
    b.addEventListener("click", () => {
      path = (b.getAttribute("data-p") as any) || "both";
      draw();
    });
  });
  host.querySelector("#df-code")!.addEventListener("click", () => {
    // 第12章：android_main 入口（块内行 2..7）
    highlightLines("android-main", [[2, 7]]);
  });

  draw();
}
