// 帧循环时序可视化：动画展示一帧 Vulkan 渲染的
// acquire → record → submit → present，以及 image_available / render_finished
// 信号量与 per-frame-in-flight 的轮转。支持参数实时调节、单步引导、代码联动。

import { highlightLines } from "../highlight";

export function mountFrameLoop(host: HTMLElement): void {
  host.innerHTML = `
    <div class="viz">
      <div class="viz-head"><span class="dot"></span> 交互演示：Vulkan 帧循环时序（acquire → record → submit → present）</div>
      <div class="viz-body">
        <canvas id="fl-canvas" width="820" height="380"></canvas>
        <div class="ctrl-row">
          <label>FRAMES_IN_FLIGHT</label>
          <input type="range" id="fl-frames" min="1" max="3" step="1" value="2" />
          <span class="val" id="fl-frames-v">2</span>
        </div>
        <div class="ctrl-row">
          <label>swapchain 图像数</label>
          <input type="range" id="fl-images" min="2" max="4" step="1" value="3" />
          <span class="val" id="fl-images-v">3</span>
        </div>
        <div class="ctrl-row">
          <label>动画速度</label>
          <input type="range" id="fl-speed" min="0.2" max="3" step="0.1" value="1" />
          <span class="val" id="fl-speed-v">1.0×</span>
        </div>
        <div class="viz-controls">
          <button id="fl-play" class="primary">暂停</button>
          <button id="fl-step">单步 ▶</button>
          <button id="fl-reset">重置</button>
          <button id="fl-code">对照第6章代码</button>
        </div>
        <div class="viz-hint">点「单步」逐步观察：acquire 返回 image index 触发 image_available；submit 后由该 image 专属的 render_finished 信号量发信号；present 等待它上屏。注意 render_finished 按 <b>图像索引</b> 而非帧索引轮转——这是避免验证层报错的关键。</div>
      </div>
    </div>`;

  const canvas = host.querySelector<HTMLCanvasElement>("#fl-canvas")!;
  const ctx = canvas.getContext("2d")!;
  const W = canvas.width;
  const H = canvas.height;

  const playBtn = host.querySelector<HTMLButtonElement>("#fl-play")!;
  const stepBtn = host.querySelector<HTMLButtonElement>("#fl-step")!;
  const resetBtn = host.querySelector<HTMLButtonElement>("#fl-reset")!;
  const codeBtn = host.querySelector<HTMLButtonElement>("#fl-code")!;
  const framesInput = host.querySelector<HTMLInputElement>("#fl-frames")!;
  const imagesInput = host.querySelector<HTMLInputElement>("#fl-images")!;
  const speedInput = host.querySelector<HTMLInputElement>("#fl-speed")!;

  const PHASES = ["acquire", "record", "submit", "present"] as const;
  type Phase = (typeof PHASES)[number];

  let framesInFlight = 2; // 真实源码 = 2
  let imageCount = 3; // swapchain 图像数（通常 3）
  let speed = 1;
  let frame = 0;
  let phaseIdx = 0;
  let playing = true;
  let t = 0;
  let last = performance.now();

  const imageIndex = (f: number) => f % imageCount;

  function draw() {
    ctx.clearRect(0, 0, W, H);
    ctx.fillStyle = "#080b10";
    ctx.fillRect(0, 0, W, H);

    const laneY = [72, 138, 208, 280];
    const labels = [
      "CPU (主线程)",
      "GPU (图形队列)",
      "image_available",
      "render_finished",
    ];
    ctx.font = "13px ui-monospace, monospace";
    labels.forEach((lb, i) => {
      ctx.strokeStyle = "#232c3a";
      ctx.beginPath();
      ctx.moveTo(20, laneY[i] + 18);
      ctx.lineTo(W - 20, laneY[i] + 18);
      ctx.stroke();
      ctx.fillStyle = "#9fb0c2";
      ctx.fillText(lb, 20, laneY[i] + 8);
    });

    const x0 = 150;
    const x1 = W - 40;
    const span = x1 - x0;

    for (let f = Math.max(0, frame - 3); f <= frame + 1; f++) {
      const x = x0 + ((f % 4) / 4) * span;
      const w = span / 4 - 6;
      if (w <= 0) continue;
      const isCurrent = f === frame;
      const alpha = isCurrent ? 1 : 0.32;
      ctx.globalAlpha = alpha;

      // CPU: acquire(蓝) + record(绿) + submit(橙)
      const cpuX = x;
      const cpuW = w * 0.55;
      ctx.fillStyle = "#58a6ff";
      ctx.fillRect(cpuX, laneY[0] + 4, cpuW, 12);
      ctx.fillStyle = "#7ee787";
      ctx.fillRect(cpuX + cpuW, laneY[0] + 4, w * 0.2, 12);
      ctx.fillStyle = "#ff9d6f";
      ctx.fillRect(cpuX + cpuW + w * 0.2, laneY[0] + 4, w * 0.25, 12);

      // GPU: 执行（submit 后）
      ctx.fillStyle = "#7ee787";
      const gx = cpuX + cpuW + w * 0.1;
      ctx.fillRect(gx, laneY[1] + 4, w * 0.5, 12);

      // image_available（per-frame 轮转）
      const fi = f % framesInFlight;
      const curFi = frame % framesInFlight;
      ctx.fillStyle = fi === curFi ? "#58a6ff" : "#2f3a48";
      ctx.beginPath();
      ctx.arc(x + w / 2, laneY[2] + 10, 6, 0, Math.PI * 2);
      ctx.fill();

      // render_finished（per-image）
      const ii = imageIndex(f);
      const curIi = imageIndex(frame);
      ctx.fillStyle = ii === curIi && isCurrent ? "#ff6b6b" : "#4a3030";
      ctx.beginPath();
      ctx.arc(x + w / 2, laneY[3] + 10, 6, 0, Math.PI * 2);
      ctx.fill();

      ctx.globalAlpha = 1;
      ctx.fillStyle = "#61718a";
      ctx.font = "11px ui-monospace, monospace";
      ctx.fillText(`帧${f}`, x, laneY[0] - 2);
      ctx.fillStyle = isCurrent ? "#e8eef6" : "#61718a";
      ctx.fillText(`img${ii}`, x + w / 2 - 8, laneY[3] + 32);
    }

    const phase = PHASES[phaseIdx];
    const phaseText: Record<Phase, string> = {
      acquire: "① acquireNextImageKHR → 拿到 image index，触发 image_available",
      record: "② 录制命令缓冲（清屏 / 绘制）",
      submit: "③ vkQueueSubmit → 等待 image_available，发信号 render_finished[img]",
      present: "④ vkQueuePresentKHR → 等待 render_finished[img]，上屏",
    };
    ctx.fillStyle = "#e8eef6";
    ctx.font = "14px ui-monospace, monospace";
    ctx.fillText(phaseText[phase], 24, H - 26);

    ctx.fillStyle = "#9fb0c2";
    ctx.font = "12px ui-monospace, monospace";
    ctx.fillText(
      `frame=${frame}  acquireSem[${frame % framesInFlight}]  renderFinished[img ${imageIndex(
        frame
      )}]  (图像数 ${imageCount})`,
      24,
      H - 8
    );
  }

  function advance() {
    t += 0.04 * speed;
    if (t >= 1) {
      t = 0;
      phaseIdx++;
      if (phaseIdx >= PHASES.length) {
        phaseIdx = 0;
        frame++;
      }
    }
  }

  function loop(now: number) {
    const dt = now - last;
    last = now;
    if (playing && dt > 0) advance();
    draw();
    requestAnimationFrame(loop);
  }

  playBtn.addEventListener("click", () => {
    playing = !playing;
    playBtn.textContent = playing ? "暂停" : "播放";
  });
  stepBtn.addEventListener("click", () => {
    playing = false;
    playBtn.textContent = "播放";
    advance();
    draw();
  });
  resetBtn.addEventListener("click", () => {
    frame = 0;
    phaseIdx = 0;
    t = 0;
    playing = true;
    playBtn.textContent = "暂停";
  });
  codeBtn.addEventListener("click", () => {
    // 第6章伪代码：acquire(行35-36) / submit(行41-46) / present(行49)
    highlightLines("frame-loop", [
      [35, 36],
      [41, 46],
      [49, 49],
    ]);
  });

  framesInput.addEventListener("input", () => {
    framesInFlight = +framesInput.value;
    host.querySelector("#fl-frames-v")!.textContent = framesInput.value;
  });
  imagesInput.addEventListener("input", () => {
    imageCount = +imagesInput.value;
    host.querySelector("#fl-images-v")!.textContent = imagesInput.value;
  });
  speedInput.addEventListener("input", () => {
    speed = +speedInput.value;
    host.querySelector("#fl-speed-v")!.textContent = `${(+speedInput.value).toFixed(
      1
    )}×`;
  });

  requestAnimationFrame(loop);
}
