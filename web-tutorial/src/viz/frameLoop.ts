// 帧循环时序可视化：动画展示一帧 Vulkan 渲染的 acquire → record → submit → present，
// 以及 image_available / render_finished 信号量与 per-frame-in-flight 的轮转。

export function mountFrameLoop(host: HTMLElement): void {
  host.innerHTML = `
    <div class="viz">
      <div class="viz-head"><span class="dot"></span> 交互演示：Vulkan 帧循环时序（acquire → record → submit → present）</div>
      <div class="viz-body">
        <canvas id="fl-canvas" width="820" height="360"></canvas>
        <div class="viz-controls">
          <button id="fl-play">暂停</button>
          <button id="fl-step">单步</button>
          <button id="fl-reset">重置</button>
        </div>
        <div class="viz-hint">点击「单步」逐步观察：CPU 提交命令、GPU 执行、信号量在帧之间如何轮转。红点为 per-image 的 render_finished 信号量。</div>
      </div>
    </div>`;

  const canvas = host.querySelector<HTMLCanvasElement>("#fl-canvas")!;
  const ctx = canvas.getContext("2d")!;
  const playBtn = host.querySelector<HTMLButtonElement>("#fl-play")!;
  const stepBtn = host.querySelector<HTMLButtonElement>("#fl-step")!;
  const resetBtn = host.querySelector<HTMLButtonElement>("#fl-reset")!;

  const W = canvas.width;
  const H = canvas.height;
  const MAX_FRAMES = 2; // MAX_FRAMES_IN_FLIGHT

  // 阶段定义
  const PHASES = ["acquire", "record", "submit", "present"] as const;
  type Phase = (typeof PHASES)[number];

  let frame = 0; // 逻辑帧计数
  let phaseIdx = 0;
  let playing = true;
  let t = 0; // 当前阶段内进度 0..1
  let last = performance.now();

  function imageIndex(f: number): number {
    // 模拟 acquire 返回的 image index（可能重复）
    return f % 3;
  }

  function draw() {
    ctx.clearRect(0, 0, W, H);
    // 背景
    ctx.fillStyle = "#0a0d12";
    ctx.fillRect(0, 0, W, H);

    const laneY = [70, 130, 200, 270];
    const labels = ["CPU (主线程)", "GPU (图形队列)", "image_available", "render_finished"];

    // 泳道
    ctx.font = "13px ui-monospace, monospace";
    labels.forEach((lb, i) => {
      ctx.strokeStyle = "#232b36";
      ctx.beginPath();
      ctx.moveTo(20, laneY[i] + 18);
      ctx.lineTo(W - 20, laneY[i] + 18);
      ctx.stroke();
      ctx.fillStyle = "#9aa7b4";
      ctx.fillText(lb, 20, laneY[i] + 8);
    });

    const x0 = 150;
    const x1 = W - 40;
    const span = x1 - x0;

    // 画最近几帧的条带
    for (let f = Math.max(0, frame - 3); f <= frame + 1; f++) {
      const x = x0 + ((f % 4) / 4) * span;
      const w = span / 4 - 6;
      if (w <= 0) continue;
      const isCurrent = f === frame;
      const alpha = isCurrent ? 1 : 0.35;
      ctx.globalAlpha = alpha;

      // CPU: acquire + record + submit
      const cpuX = x;
      const cpuW = w * 0.55;
      ctx.fillStyle = "#5fb3ff";
      ctx.fillRect(cpuX, laneY[0] + 4, cpuW, 12);
      ctx.fillStyle = "#7ee787";
      ctx.fillRect(cpuX + cpuW, laneY[0] + 4, w * 0.2, 12);
      ctx.fillStyle = "#ff9d6f";
      ctx.fillRect(cpuX + cpuW + w * 0.2, laneY[0] + 4, w * 0.25, 12);

      // GPU: 执行（submit 后）
      ctx.fillStyle = "#7ee787";
      const gx = cpuX + cpuW + w * 0.1;
      ctx.fillRect(gx, laneY[1] + 4, w * 0.5, 12);

      // image_available 信号量（per-frame 轮转）
      const fi = f % MAX_FRAMES;
      ctx.fillStyle = fi === frame % MAX_FRAMES ? "#5fb3ff" : "#3a4654";
      ctx.beginPath();
      ctx.arc(x + w / 2, laneY[2] + 10, 6, 0, Math.PI * 2);
      ctx.fill();

      // render_finished 信号量（per-image）
      const ii = imageIndex(f);
      ctx.fillStyle = ii === imageIndex(frame) && isCurrent ? "#ff6b6b" : "#4a3a3a";
      ctx.beginPath();
      ctx.arc(x + w / 2, laneY[3] + 10, 6, 0, Math.PI * 2);
      ctx.fill();

      ctx.globalAlpha = 1;
      ctx.fillStyle = "#6b7785";
      ctx.fillText(`帧${f}`, x, laneY[0] - 2);
    }

    // 当前阶段高亮文字
    const phase = PHASES[phaseIdx];
    const phaseText: Record<Phase, string> = {
      acquire: "① acquireNextImageKHR → 拿到 image index，触发 image_available",
      record: "② 录制命令缓冲（清屏 / 绘制）",
      submit: "③ vkQueueSubmit → 等待 image_available，发信号 render_finished",
      present: "④ vkQueuePresentKHR → 等待 render_finished，上屏",
    };
    ctx.fillStyle = "#e6edf3";
    ctx.font = "14px ui-monospace, monospace";
    ctx.fillText(phaseText[phase], 24, H - 24);

    // 帧数 / 信号量状态
    ctx.fillStyle = "#9aa7b4";
    ctx.font = "12px ui-monospace, monospace";
    ctx.fillText(
      `frame=${frame}  acquireSem[${frame % MAX_FRAMES}]  renderFinished[image ${imageIndex(
        frame
      )}]`,
      24,
      H - 6
    );
  }

  function advance() {
    t += 0.04;
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
    if (playing) {
      // 用 dt 控制节奏
      if (dt > 0) advance();
    }
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

  requestAnimationFrame(loop);
}
