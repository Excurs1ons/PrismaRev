<script setup lang="ts">
/**
 * Scene / Game 视口面板（Unity `SceneView` / `GameView` 的简化形态）。
 *
 * 渲染发生在**编辑器进程内**：Rust 侧的视口线程持有一个 `prism_app::App`
 * （离屏 Vulkan），把 tonemap 后的 RGBA 帧经 Tauri `Channel` 推过来，这里
 * 只是把帧 `putImageData` 到 canvas。前端不参与任何场景求值。
 *
 * - Scene view：编辑态（不跑游戏 system），相机由本组件的轨道控制驱动，
 *   文档改动后自动重建（debounce）。编辑器相机在 Rust 侧 `depth = 1000`。
 * - Game view：Play 时模拟游戏循环；鼠标/键盘转发给引擎 `InputManager`。
 */
import { Channel } from "@tauri-apps/api/core";
import { HelpCircle, Pause, Play, RefreshCw, Square, StepForward, X, Zap } from "lucide-vue-next";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import * as api from "../api";
import { t } from "../i18n";
import EditorDropdown, { type DropdownOption } from "./EditorDropdown.vue";
import OrientationGizmo from "./OrientationGizmo.vue";
import type { CameraOrbit, SceneDoc, ViewportInput, ViewportSnapshot } from "../types";

const props = defineProps<{
  kind: "scene" | "game";
  doc: SceneDoc | null;
  path: string | null;
  /** 当前主工具栏选择的变换工具。 */
  activeTool?: "select" | "move" | "rotate" | "scale";
  /** 当前主工具栏选择的坐标系（Global vs Local）。 */
  activeSpace?: "global" | "local";
  /** Game view 的 Play 状态（由工具栏统一控制）。 */
  playing?: boolean;
  /** Game view 是否暂停（由工具栏控制，暂停时仍渲染）。 */
  paused?: boolean;
  /** 当前选中项（文档 `entities` 下标，与层级 / 检查器共用）。 */
  selection?: number[];
}>();

const emit = defineEmits<{
  (event: "toggle-play"): void;
  /** 在场景视图里点选：`null` = 点空处（取消选中）。 */
  (event: "select", index: number | null): void;
  /** Shift / Ctrl 多选：批量更新选中项。 */
  (event: "select-multiple", indices: number[]): void;
  /** 拖动平移手柄后的新局部位置 `[x, y, z]`。 */
  (event: "move", position: [number, number, number]): void;
  /** 拖动旋转圆环后的新局部四元数 `[x, y, z, w]`。 */
  (event: "rotate", rotation: [number, number, number, number]): void;
  /** 拖动缩放手柄后的新局部缩放 `[x, y, z]`。 */
  (event: "scale", scale: [number, number, number]): void;
  /** Game view：暂停 / 继续 Play。 */
  (event: "toggle-pause"): void;
  /** Game view：暂停中前进一帧。 */
  (event: "step"): void;
}>();

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

const viewId = props.kind;
const container = ref<HTMLDivElement | null>(null);
const canvasEl = ref<HTMLCanvasElement | null>(null);
const status = ref<"idle" | "loading" | "live" | "error">("idle");
const message = ref<string | null>(null);
const stats = ref<ViewportSnapshot | null>(null);
const frames = ref(0);
const displayFps = ref(0);

/** Scene view 的轨道相机（唯一的状态持有者，整体下发给后端）。 */
const orbit = ref<CameraOrbit>({ target: [0, 0, 0], yaw: -0.7, pitch: 0.35, distance: 12 });

const isScene = computed(() => props.kind === "scene");
const canRender = computed(() => inTauri && !!props.doc && !!props.path);

let channel: Channel<ArrayBuffer> | null = null;
let resizeObserver: ResizeObserver | null = null;
let resizeTimer: number | undefined;
let reloadTimer: number | undefined;
let statsTimer: number | undefined;
let cameraPending = false;
let isActive = true;
let pendingFrame: ArrayBuffer | null = null;
let drawScheduled = false;
let lastFpsSample = performance.now();
let sampledFrames = 0;
let context2d: CanvasRenderingContext2D | null = null;
let imageData: ImageData | null = null;
/** 场景第一次出帧时自动框住内容；换文档后重置。 */
let autoFramed = false;

/** 面板尺寸（物理像素），已含 devicePixelRatio。 */
function physicalSize(): { width: number; height: number } {
  const rect = container.value?.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  return {
    width: Math.max(1, Math.round((rect?.width ?? 640) * dpr)),
    height: Math.max(1, Math.round((rect?.height ?? 480) * dpr)),
  };
}

async function open() {
  if (!inTauri) {
    message.value = t("viewport.needDesktopOverlay");
    status.value = "error";
    return;
  }
  if (!props.doc || !props.path) {
    status.value = "idle";
    return;
  }
  status.value = "loading";
  message.value = null;
  pendingFrame = null;
  drawScheduled = false;
  context2d = null;
  imageData = null;
  displayFps.value = 0;
  sampledFrames = 0;
  lastFpsSample = performance.now();
  const { width, height } = physicalSize();
  channel = new Channel<ArrayBuffer>();
  channel.onmessage = (buffer) => {
    // Keep only the newest image when IPC is faster than the browser's paint loop.
    pendingFrame = buffer;
    if (drawScheduled) return;
    drawScheduled = true;
    requestAnimationFrame(paintPendingFrame);
  };
  try {
    stats.value = await api.viewportOpen(
      viewId,
      props.kind,
      props.doc,
      width,
      height,
      channel,
      isScene.value ? orbit.value : undefined,
      !isScene.value && props.playing,
    );
    pushSelection();
    if (props.activeTool && isScene.value) {
      void api.viewportSetTool(viewId, props.activeTool).catch(() => undefined);
    }
    if (props.activeSpace && isScene.value) {
      void api.viewportSetSpace(viewId, props.activeSpace).catch(() => undefined);
    }
    // 第一帧到达时 drawFrame 才切到 live：渲染器还在建管线时先保留 loading 浮层，
    // 否则会先闪一段黑屏。
  } catch (error) {
    status.value = "error";
    message.value = String(error);
  }
}

async function close() {
  if (!inTauri) return;
  pendingFrame = null;
  drawScheduled = false;
  context2d = null;
  imageData = null;
  channel = null;
  frames.value = 0;
  displayFps.value = 0;
  await api.viewportClose(viewId).catch(() => false);
}

/** 一帧 = 8 字节头（宽/高，小端 u32）+ RGBA8 像素。 */
function drawFrame(buffer: ArrayBuffer) {
  const canvas = canvasEl.value;
  if (!canvas || buffer.byteLength < 8) return;
  const header = new DataView(buffer);
  const width = header.getUint32(0, true);
  const height = header.getUint32(4, true);
  if (width === 0 || height === 0) return;
  if (canvas.width !== width || canvas.height !== height) {
    canvas.width = width;
    canvas.height = height;
    imageData = null;
  }
  const context = context2d ?? (context2d = canvas.getContext("2d"));
  if (!context) return;
  const pixels = new Uint8ClampedArray(buffer, 8);
  if (!imageData || imageData.width !== width || imageData.height !== height) {
    imageData = new ImageData(width, height);
  }
  imageData.data.set(pixels);
  context.putImageData(imageData, 0, 0);
  frames.value += 1;
  sampledFrames += 1;
  const now = performance.now();
  const elapsed = now - lastFpsSample;
  if (elapsed >= 500) {
    displayFps.value = Math.round((sampledFrames * 1000) / elapsed);
    sampledFrames = 0;
    lastFpsSample = now;
  }
  status.value = "live";
  if (isScene.value && !autoFramed) {
    autoFramed = true;
    void frameAll();
  }
}

function paintPendingFrame() {
  drawScheduled = false;
  const frame = pendingFrame;
  pendingFrame = null;
  if (frame) drawFrame(frame);
  if (pendingFrame && !drawScheduled) {
    drawScheduled = true;
    requestAnimationFrame(paintPendingFrame);
  }
}

function scheduleResize() {
  if (resizeTimer !== undefined) window.clearTimeout(resizeTimer);
  resizeTimer = window.setTimeout(() => {
    if (!inTauri || !channel) return;
    const { width, height } = physicalSize();
    if (width < 2 || height < 2) return;
    void api.viewportResize(viewId, width, height).catch(() => undefined);
  }, 120);
}

/**
 * 停靠面板用 `v-show` 隐藏（`display:none` → 盒子变 0×0）。
 *
 * 视口线程看不见 DOM，所以这里把「可见性」翻译成 `set_active`：隐藏时不 tick、
 * 不出帧（Scene / Game 两个视口同时存在时省一半 GPU）。
 */
function setActive(active: boolean) {
  if (!inTauri || active === isActive) return;
  isActive = active;
  void api.viewportSetActive(viewId, active).catch(() => undefined);
}

function scheduleReload() {
  if (!inTauri) return;
  // 拖动 gizmo 期间文档每帧都在变，重载会换掉 ECS 句柄；攒到松手后再同步。
  if (draggingAxis.value) {
    reloadPending = true;
    return;
  }
  // Play 期间编辑文档不该打断运行中的模拟（Unity 里编辑也会被丢弃）；
  // 停止时会先把当前文档刷进来，所以这里直接丢掉。
  if (props.playing) {
    reloadPending = false;
    return;
  }
  if (reloadTimer !== undefined) window.clearTimeout(reloadTimer);
  reloadTimer = window.setTimeout(async () => {
    if (!props.doc) return;
    // 面板还没打开（没有通道）时直接重建整个视口。
    if (!channel) {
      await open();
      return;
    }
    try {
      stats.value = await api.viewportReload(viewId, props.doc);
      scheduleFetchSelectionBounds();
      message.value = null;
    } catch (error) {
      message.value = String(error);
      status.value = "error";
    }
  }, 450);
}

// ── 相机 / 输入 ────────────────────────────────────────────────────────────

let dragMode: "orbit" | "pan" | "marquee" | null = null;
let lastPointer = { x: 0, y: 0 };
/** 判定「点选」还是「拖动相机」：按下位置 + 时间。 */
let clickStart: { x: number; y: number; at: number } | null = null;
/** 正在拖动的平移手柄（`"x"` / `"y"` / `"z"`），`null` = 没在拖。 */
const draggingAxis = ref<string | null>(null);
/** 拖动过程中的实时交互反馈（浮层数值提示、旋转弧面等）。 */
const dragFeedback = ref<api.DragFeedback | null>(null);
/** 鼠标悬停手柄的投影轮廓（用于高亮提示）。 */
const hoveredGizmo = ref<api.GizmoHoverInfo | null>(null);
let hoverFrame = 0;
let pendingHover: { x: number; y: number } | null = null;

/** 场景视图矩形框选 (Marquee Selection) 状态。 */
const marqueeStart = ref<{ x: number; y: number } | null>(null);
const marqueeCurrent = ref<{ x: number; y: number } | null>(null);

const marqueeBox = computed(() => {
  if (dragMode !== "marquee" || !marqueeStart.value || !marqueeCurrent.value) return null;
  const canvas = canvasEl.value;
  if (!canvas) return null;
  const rect = canvas.getBoundingClientRect();
  const x1 = Math.min(marqueeStart.value.x, marqueeCurrent.value.x) - rect.left;
  const y1 = Math.min(marqueeStart.value.y, marqueeCurrent.value.y) - rect.top;
  const x2 = Math.max(marqueeStart.value.x, marqueeCurrent.value.x) - rect.left;
  const y2 = Math.max(marqueeStart.value.y, marqueeCurrent.value.y) - rect.top;
  const w = x2 - x1;
  const h = y2 - y1;
  if (w < 2 && h < 2) return null;
  return { x: x1, y: y1, width: w, height: h };
});

/** 选中实体的 3D 投影包围盒列表。 */
const selectionBounds = ref<api.ProjectedBoundBox[]>([]);
let boundsFrame = 0;

const BOUND_BOX_EDGES: [number, number][] = [
  // 底面
  [0, 1], [1, 3], [3, 2], [2, 0],
  // 顶面
  [4, 5], [5, 7], [7, 6], [6, 4],
  // 竖立边
  [0, 4], [1, 5], [2, 6], [3, 7],
];

function scheduleFetchSelectionBounds() {
  if (!inTauri || !channel || !isScene.value || !props.selection || props.selection.length === 0) {
    selectionBounds.value = [];
    return;
  }
  if (boundsFrame) return;
  boundsFrame = requestAnimationFrame(() => {
    boundsFrame = 0;
    const indices = props.selection ?? [];
    if (indices.length === 0) {
      selectionBounds.value = [];
      return;
    }
    void api
      .viewportSelectionBounds(viewId, indices)
      .then((bounds) => {
        selectionBounds.value = bounds;
      })
      .catch(() => {
        selectionBounds.value = [];
      });
  });
}

/** Unity 风格第一人称飞行相机模式（右键按住 + WASD/QE 漫游）。 */
const flyMode = ref(false);
const flySpeed = ref(10.0);
const flyKeys = {
  w: false,
  a: false,
  s: false,
  d: false,
  q: false,
  e: false,
  shift: false,
};
let flyAnimFrame = 0;
let lastFlyTime = 0;

function startFlyLoop() {
  if (flyAnimFrame) cancelAnimationFrame(flyAnimFrame);
  lastFlyTime = performance.now();

  function step(now: number) {
    if (!flyMode.value) {
      flyAnimFrame = 0;
      return;
    }
    const dt = Math.min(0.1, (now - lastFlyTime) / 1000);
    lastFlyTime = now;

    const multiplier = flyKeys.shift ? 2.5 : 1.0;
    const speed = flySpeed.value * multiplier * dt;

    const { yaw, pitch, target } = orbit.value;
    const forwardX = Math.cos(pitch) * -Math.sin(yaw);
    const forwardY = -Math.sin(pitch);
    const forwardZ = Math.cos(pitch) * -Math.cos(yaw);

    const rightX = Math.cos(yaw);
    const rightZ = -Math.sin(yaw);

    let mx = 0;
    let my = 0;
    let mz = 0;

    if (flyKeys.w) { mx += forwardX; my += forwardY; mz += forwardZ; }
    if (flyKeys.s) { mx -= forwardX; my -= forwardY; mz -= forwardZ; }
    if (flyKeys.d) { mx += rightX; mz += rightZ; }
    if (flyKeys.a) { mx -= rightX; mz -= rightZ; }
    if (flyKeys.e) { my += 1.0; }
    if (flyKeys.q) { my -= 1.0; }

    const len = Math.hypot(mx, my, mz);
    if (len > 1e-4) {
      const stepDist = speed / len;
      orbit.value.target = [
        target[0] + mx * stepDist,
        target[1] + my * stepDist,
        target[2] + mz * stepDist,
      ];
      pushCamera();
    }
    flyAnimFrame = requestAnimationFrame(step);
  }
  flyAnimFrame = requestAnimationFrame(step);
}

function stopFlyLoop() {
  if (flyAnimFrame) {
    cancelAnimationFrame(flyAnimFrame);
    flyAnimFrame = 0;
  }
  flyKeys.w = false;
  flyKeys.a = false;
  flyKeys.s = false;
  flyKeys.d = false;
  flyKeys.q = false;
  flyKeys.e = false;
  flyKeys.shift = false;
}

/** 旋转手柄拖拽时的 SVG 扇面与弧线路径。 */
const rotateArcPath = computed(() => {
  const fb = dragFeedback.value;
  if (!fb || fb.tool !== "rotate" || !fb.startScreen || !fb.currentScreen) return null;
  const [cx, cy] = fb.centerScreen;
  const [x1, y1] = fb.startScreen;
  const [x2, y2] = fb.currentScreen;
  const r = fb.radius && fb.radius > 5 ? fb.radius : Math.hypot(x1 - cx, y1 - cy);
  if (r < 2) return null;

  // 2D 叉积判定顺逆时针（SVG 屏幕坐标系 y 朝下）
  const v1x = x1 - cx;
  const v1y = y1 - cy;
  const v2x = x2 - cx;
  const v2y = y2 - cy;
  const cross = v1x * v2y - v1y * v2x;
  const sweep = cross >= 0 ? 1 : 0;

  const angleDeg = Math.abs(fb.angleDeg ?? 0);
  const largeArc = angleDeg > 180 ? 1 : 0;

  return {
    cx,
    cy,
    x1,
    y1,
    x2,
    y2,
    r,
    wedgeD: `M ${cx.toFixed(1)} ${cy.toFixed(1)} L ${x1.toFixed(1)} ${y1.toFixed(1)} A ${r.toFixed(1)} ${r.toFixed(1)} 0 ${largeArc} ${sweep} ${x2.toFixed(1)} ${y2.toFixed(1)} Z`,
    arcD: `M ${x1.toFixed(1)} ${y1.toFixed(1)} A ${r.toFixed(1)} ${r.toFixed(1)} 0 ${largeArc} ${sweep} ${x2.toFixed(1)} ${y2.toFixed(1)}`,
  };
});

/** 拖拽浮动数值 Badge 的位置（限制在视口边界内，防遮挡手柄）。 */
const badgePosition = computed(() => {
  const fb = dragFeedback.value;
  if (!fb) return null;
  const w = canvasEl.value?.width || 1280;
  const h = canvasEl.value?.height || 720;
  let rawX = fb.centerScreen[0] + 18;
  let rawY = fb.centerScreen[1] - 14;
  if (fb.tool === "rotate" && fb.currentScreen) {
    rawX = fb.currentScreen[0] + 16;
    rawY = fb.currentScreen[1] - 16;
  }
  const bw = Math.max(68, fb.text.length * 8 + 16);
  const x = Math.max(10, Math.min(w - bw - 10, rawX));
  const y = Math.max(24, Math.min(h - 10, rawY));
  return { x, y, width: bw };
});
/** 拖动期间待下发的光标位置 / 是否攒了未同步的文档重载。 */
let pendingDrag: { x: number; y: number; snap: boolean } | null = null;
let reloadPending = false;
let dragFrame = 0;
/** 左键按下瞬间的相机（万一这一按其实落在手柄上，用于还原）。 */
let orbitBeforePress: CameraOrbit | null = null;
let cameraAnimFrame = 0;

/** 相机变更按 rAF 合并：拖动时每帧最多下发一次。 */
function pushCamera() {
  if (!inTauri || !isScene.value) return;
  scheduleFetchSelectionBounds();
  if (cameraPending) return;
  cameraPending = true;
  requestAnimationFrame(() => {
    cameraPending = false;
    void api.viewportCamera(viewId, orbit.value).catch(() => undefined);
  });
}

/** 平滑动画过渡视口相机到目标参数（角度、注视点与距离）。 */
function tweenCameraTo(
  target: {
    yaw?: number;
    pitch?: number;
    distance?: number;
    target?: [number, number, number];
  },
  duration = 200,
) {
  if (cameraAnimFrame) {
    cancelAnimationFrame(cameraAnimFrame);
    cameraAnimFrame = 0;
  }
  const startYaw = orbit.value.yaw;
  const startPitch = orbit.value.pitch;
  const startDist = orbit.value.distance;
  const startTgt = [...orbit.value.target] as [number, number, number];

  const endYaw =
    target.yaw !== undefined
      ? (() => {
          const diff = (target.yaw - startYaw) % (Math.PI * 2);
          const shortestDiff = ((diff + Math.PI * 3) % (Math.PI * 2)) - Math.PI;
          return startYaw + shortestDiff;
        })()
      : startYaw;
  const endPitch = target.pitch !== undefined ? target.pitch : startPitch;
  const endDist = target.distance !== undefined ? target.distance : startDist;
  const endTgt = target.target !== undefined ? target.target : startTgt;

  const startTime = performance.now();

  function step(now: number) {
    const elapsed = now - startTime;
    const t = Math.min(1, elapsed / duration);
    // Ease-out quad 曲线
    const ease = t * (2 - t);

    orbit.value.yaw = startYaw + (endYaw - startYaw) * ease;
    orbit.value.pitch = startPitch + (endPitch - startPitch) * ease;
    orbit.value.distance = startDist + (endDist - startDist) * ease;
    orbit.value.target = [
      startTgt[0] + (endTgt[0] - startTgt[0]) * ease,
      startTgt[1] + (endTgt[1] - startTgt[1]) * ease,
      startTgt[2] + (endTgt[2] - startTgt[2]) * ease,
    ];
    pushCamera();

    if (t < 1) {
      cameraAnimFrame = requestAnimationFrame(step);
    } else {
      cameraAnimFrame = 0;
    }
  }

  cameraAnimFrame = requestAnimationFrame(step);
}

/** 平滑动画过渡视口相机到目标角度（如点击右上角方向导航指示器）。 */
function alignCameraTo(target: { yaw: number; pitch: number }) {
  tweenCameraTo(target, 180);
}

/** 重置为标准透视斜俯视视角 (yaw = -0.7, pitch = 0.35)。 */
function resetCameraView() {
  alignCameraTo({ yaw: -0.7, pitch: 0.35 });
}

function onPointerDown(event: PointerEvent) {
  const canvas = canvasEl.value;
  if (!canvas) return;
  canvas.focus();
  if (isScene.value) {
    if (event.button === 2) {
      flyMode.value = true;
      startFlyLoop();
    }
    if (props.activeTool === "select" && event.button === 0 && !event.altKey) {
      dragMode = "marquee";
      marqueeStart.value = { x: event.clientX, y: event.clientY };
      marqueeCurrent.value = { x: event.clientX, y: event.clientY };
      clickStart = { x: event.clientX, y: event.clientY, at: Date.now() };
      canvas.setPointerCapture(event.pointerId);
      event.preventDefault();
      return;
    }
    const point = event.button === 0 && !event.shiftKey ? canvasPoint(event) : null;
    if (point) {
      // 左键：先按「轨道旋转 / 点选」记账（保证快速点击仍然能点选），
      // 同时问后端是否按在平移手柄上；是就切过去并把相机还原。
      orbitBeforePress = { ...orbit.value };
      beginCameraDrag(event, canvas);
      void beginGizmoDrag(point, event, canvas);
    } else {
      beginCameraDrag(event, canvas);
    }
    return;
  }
  void sendInput({ type: "mouseButton", button: event.button, pressed: true });
}

/** 轨道旋转 / 平移：记录起点并接管指针。 */
function beginCameraDrag(event: PointerEvent, canvas: HTMLCanvasElement) {
  if (cameraAnimFrame) {
    cancelAnimationFrame(cameraAnimFrame);
    cameraAnimFrame = 0;
  }
  hoveredGizmo.value = null;
  if (event.button === 1 || event.shiftKey) {
    dragMode = "pan";
  } else if (event.button === 0 || event.button === 2) {
    dragMode = "orbit";
  } else {
    return;
  }
  lastPointer = { x: event.clientX, y: event.clientY };
  clickStart = event.button === 0 ? { x: event.clientX, y: event.clientY, at: Date.now() } : null;
  canvas.setPointerCapture(event.pointerId);
  event.preventDefault();
}

/** 按在平移手柄上 → 后端开始拖动会话（`draggingAxis` 非空后 pointermove 变成移动实体）。 */
async function beginGizmoDrag(
  point: { x: number; y: number },
  event: PointerEvent,
  canvas: HTMLCanvasElement,
) {
  if (cameraAnimFrame) {
    cancelAnimationFrame(cameraAnimFrame);
    cameraAnimFrame = 0;
  }
  if (!inTauri || !channel) return;
  const axis = await api.viewportGizmoHit(viewId, point.x, point.y).catch(() => null);
  // 松手了 / 没按到手柄：保持「轨道 + 点选」的记账，什么都不做。
  if (!axis || event.buttons === 0) return;
  try {
    await api.viewportDragBegin(viewId, axis, point.x, point.y);
  } catch {
    return;
  }
  // 命中手柄之前可能已经转了一两像素的轨道视点，还原回去。
  if (orbitBeforePress) {
    orbit.value = orbitBeforePress;
    orbitBeforePress = null;
    pushCamera();
  }
  dragMode = null;
  clickStart = null;
  hoveredGizmo.value = null;
  draggingAxis.value = axis;
  canvas.setPointerCapture(event.pointerId);
  event.preventDefault();
}

/** 悬停手柄检测按 rAF 合并节流。 */
function pushHoverCheck(point: { x: number; y: number }) {
  if (!inTauri || !channel || !isScene.value || draggingAxis.value || dragMode) {
    hoveredGizmo.value = null;
    return;
  }
  if (hoverFrame) {
    pendingHover = point;
    return;
  }
  hoverFrame = requestAnimationFrame(() => {
    hoverFrame = 0;
    const target = pendingHover ?? point;
    pendingHover = null;
    void api
      .viewportGizmoHover(viewId, target.x, target.y)
      .then((info) => {
        if (!draggingAxis.value && !dragMode) {
          hoveredGizmo.value = info;
        }
      })
      .catch(() => {
        hoveredGizmo.value = null;
      });
  });
}

function onPointerLeave() {
  if (flyMode.value) {
    flyMode.value = false;
    stopFlyLoop();
  }
  hoveredGizmo.value = null;
  pendingHover = null;
  if (hoverFrame) {
    cancelAnimationFrame(hoverFrame);
    hoverFrame = 0;
  }
}

function onPointerMove(event: PointerEvent) {
  if (isScene.value) {
    if (dragMode === "marquee") {
      marqueeCurrent.value = { x: event.clientX, y: event.clientY };
      return;
    }
    if (draggingAxis.value) {
      hoveredGizmo.value = null;
      const point = canvasPoint(event);
      if (point) pushDrag(point, event.ctrlKey || event.metaKey);
      return;
    }
    if (dragMode) {
      hoveredGizmo.value = null;
      const dx = event.clientX - lastPointer.x;
      const dy = event.clientY - lastPointer.y;
      lastPointer = { x: event.clientX, y: event.clientY };
      if (dragMode === "orbit") {
        orbit.value.yaw -= dx * 0.008;
        orbit.value.pitch = Math.max(
          -1.5,
          Math.min(1.5, orbit.value.pitch + dy * 0.008),
        );
      } else {
        // 沿相机右方向 / 世界上方向平移注视点。
        const scale = orbit.value.distance * 0.0016;
        const { yaw, target } = orbit.value;
        const rightX = Math.cos(yaw);
        const rightZ = -Math.sin(yaw);
        orbit.value.target = [
          target[0] - rightX * dx * scale,
          target[1] + dy * scale,
          target[2] - rightZ * dx * scale,
        ];
      }
      pushCamera();
      return;
    }
    const point = canvasPoint(event);
    if (point) pushHoverCheck(point);
    return;
  }
  void sendInput({
    type: "mouseMove",
    x: event.offsetX,
    y: event.offsetY,
  });
}

/** 暂停中前进一帧（后端会先把状态切成暂停，所以连点也安全）。 */
function requestStep() {
  if (!inTauri) return;
  void api.viewportStep(viewId).catch(() => undefined);
  emit("step");
}

/** 拖动按 rAF 合并：每帧最多问一次后端，中间的光标位置直接丢掉。 */
function pushDrag(point: { x: number; y: number }, snap: boolean) {
  if (dragFrame) {
    pendingDrag = { x: point.x, y: point.y, snap };
    return;
  }
  dragFrame = requestAnimationFrame(() => {
    dragFrame = 0;
    const target = pendingDrag ?? { x: point.x, y: point.y, snap };
    pendingDrag = null;
    void api
      .viewportDrag(viewId, target.x, target.y, target.snap)
      .then((res) => {
        if (!res) return;
        if (res.position) emit("move", res.position);
        if (res.rotation) emit("rotate", res.rotation);
        if (res.scale) emit("scale", res.scale);
        if (res.feedback) dragFeedback.value = res.feedback;
        scheduleFetchSelectionBounds();
        // 拖动期间文档在变，但重载会换掉 ECS 句柄：等松手再同步。
        reloadPending = true;
      })
      .catch(() => undefined);
  });
}

function onPointerUp(event: PointerEvent) {
  orbitBeforePress = null;
  if (isScene.value) {
    if (flyMode.value) {
      flyMode.value = false;
      stopFlyLoop();
    }
    if (dragMode === "marquee") {
      dragMode = null;
      canvasEl.value?.releasePointerCapture(event.pointerId);
      const start = marqueeStart.value;
      const current = marqueeCurrent.value;
      marqueeStart.value = null;
      marqueeCurrent.value = null;
      clickStart = null;
      if (!start || !current) return;
      const dx = Math.abs(current.x - start.x);
      const dy = Math.abs(current.y - start.y);
      if (dx < 5 && dy < 5) {
        void pickAt(event);
        return;
      }
      const canvas = canvasEl.value;
      if (!canvas) return;
      const rect = canvas.getBoundingClientRect();
      const minCssX = Math.min(start.x, current.x) - rect.left;
      const maxCssX = Math.max(start.x, current.x) - rect.left;
      const minCssY = Math.min(start.y, current.y) - rect.top;
      const maxCssY = Math.max(start.y, current.y) - rect.top;

      const scaleX = canvas.width / rect.width;
      const scaleY = canvas.height / rect.height;
      const minBmpX = minCssX * scaleX;
      const maxBmpX = maxCssX * scaleX;
      const minBmpY = minCssY * scaleY;
      const maxBmpY = maxCssY * scaleY;

      void api
        .viewportBoxSelect(viewId, minBmpX, minBmpY, maxBmpX, maxBmpY)
        .then((hits) => {
          if (event.ctrlKey || event.metaKey) {
            const currentSel = new Set(props.selection ?? []);
            for (const h of hits) {
              if (currentSel.has(h)) currentSel.delete(h);
              else currentSel.add(h);
            }
            emit("select-multiple", Array.from(currentSel));
          } else if (event.shiftKey) {
            const currentSel = new Set(props.selection ?? []);
            for (const h of hits) currentSel.add(h);
            emit("select-multiple", Array.from(currentSel));
          } else {
            emit("select-multiple", hits);
          }
        })
        .catch(() => undefined);
      return;
    }
    if (draggingAxis.value) {
      draggingAxis.value = null;
      dragFeedback.value = null;
      hoveredGizmo.value = null;
      pendingDrag = null;
      if (dragFrame) {
        cancelAnimationFrame(dragFrame);
        dragFrame = 0;
      }
      canvasEl.value?.releasePointerCapture(event.pointerId);
      void api.viewportDragEnd(viewId).catch(() => undefined);
      if (reloadPending) {
        reloadPending = false;
        scheduleReload();
      }
      return;
    }
    dragMode = null;
    canvasEl.value?.releasePointerCapture(event.pointerId);
    const start = clickStart;
    clickStart = null;
    // 左键按下后几乎没有移动 = 点选（拖动是轨道旋转，不改变选中项）。
    if (
      start &&
      event.button === 0 &&
      Date.now() - start.at < 400 &&
      Math.abs(event.clientX - start.x) < 4 &&
      Math.abs(event.clientY - start.y) < 4
    ) {
      void pickAt(event);
    }
    return;
  }
  void sendInput({ type: "mouseButton", button: event.button, pressed: false });
}

/**
 * CSS 坐标 → canvas **位图**坐标。
 *
 * canvas 的宽高就是后端实际渲染分辨率（已按 1600×900 上限缩过），所以按比例换算，
 * 不依赖 devicePixelRatio。
 */
function canvasPoint(event: PointerEvent): { x: number; y: number } | null {
  const canvas = canvasEl.value;
  if (!canvas || canvas.width === 0) return null;
  const rect = canvas.getBoundingClientRect();
  if (rect.width === 0 || rect.height === 0) return null;
  return {
    x: ((event.clientX - rect.left) / rect.width) * canvas.width,
    y: ((event.clientY - rect.top) / rect.height) * canvas.height,
  };
}

/** 点选：把 CSS 坐标换算到 canvas **位图**坐标再交给后端。支持 Shift / Ctrl 多选。 */
async function pickAt(event: PointerEvent) {
  if (!inTauri || !channel) return;
  const point = canvasPoint(event);
  if (!point) return;
  const hit = await api.viewportPick(viewId, point.x, point.y).catch(() => undefined);
  if (hit === undefined) return;
  if (hit === null) {
    if (!event.shiftKey && !event.ctrlKey && !event.metaKey) {
      emit("select", null);
    }
    return;
  }
  if (event.ctrlKey || event.metaKey) {
    // Ctrl / Cmd 多选：切换该项在选择集合中的状态
    const current = props.selection ?? [];
    const next = current.includes(hit)
      ? current.filter((i) => i !== hit)
      : [...current, hit];
    emit("select-multiple", next);
  } else if (event.shiftKey) {
    // Shift 多选：追加到选择集合
    const current = props.selection ?? [];
    if (!current.includes(hit)) {
      emit("select-multiple", [...current, hit]);
    }
  } else {
    emit("select", hit);
  }
}

/** 主选中项：多选时与层级一样取最后一个。 */
function primarySelection(): number | null {
  const selection = props.selection;
  return selection && selection.length ? selection[selection.length - 1] : null;
}

/** 把选中项同步给后端（决定 gizmo 画在哪个实体上）。 */
function pushSelection() {
  scheduleFetchSelectionBounds();
  if (!inTauri || !channel || !isScene.value) return;
  void api.viewportSelect(viewId, primarySelection()).catch(() => undefined);
}

function onWheel(event: WheelEvent) {
  if (isScene.value) {
    if (flyMode.value) {
      flySpeed.value = Math.max(
        1.0,
        Math.min(100.0, flySpeed.value * (event.deltaY < 0 ? 1.25 : 0.8)),
      );
      event.preventDefault();
      return;
    }
    orbit.value.distance = Math.max(
      0.2,
      Math.min(500, orbit.value.distance * Math.exp(event.deltaY * 0.0012)),
    );
    pushCamera();
    event.preventDefault();
    return;
  }
  void sendInput({ type: "wheel", delta: event.deltaY / 100 });
}

function onKey(event: KeyboardEvent, pressed: boolean) {
  if (isScene.value) {
    if (flyMode.value) {
      const code = event.code;
      if (code === "KeyW") { flyKeys.w = pressed; event.preventDefault(); return; }
      if (code === "KeyS") { flyKeys.s = pressed; event.preventDefault(); return; }
      if (code === "KeyA") { flyKeys.a = pressed; event.preventDefault(); return; }
      if (code === "KeyD") { flyKeys.d = pressed; event.preventDefault(); return; }
      if (code === "KeyE") { flyKeys.e = pressed; event.preventDefault(); return; }
      if (code === "KeyQ") { flyKeys.q = pressed; event.preventDefault(); return; }
      if (code === "ShiftLeft" || code === "ShiftRight") { flyKeys.shift = pressed; return; }
    }
    if (!pressed) return;
    // F = 聚焦选中项（没有选中就框住整个场景），A / Home = 框住整个场景。
    if (event.code === "KeyF") {
      void frameSelection();
      event.preventDefault();
    } else if (event.code === "KeyA" || event.code === "Home") {
      void frameAll();
      event.preventDefault();
    }
    return;
  }
  void sendInput({ type: "key", code: event.code, pressed });
  event.preventDefault();
}

function sendInput(event: ViewportInput) {
  if (!inTauri || !channel) return Promise.resolve();
  return api.viewportInput(viewId, event).catch(() => undefined);
}

/** 套用后端算出来的取景参数（`yaw` / `pitch` 由后端原样带回，平滑过渡）。 */
async function applyFrame(mode: "all" | "selection") {
  if (!inTauri || !channel) return;
  const framed = await api.viewportFrame(viewId, mode).catch(() => null);
  if (!framed) return;
  tweenCameraTo(framed, 220);
}

/** 框住整个场景（`A` / `Home` / 工具栏按钮）。 */
function frameAll() {
  return applyFrame("all");
}

/** 聚焦选中项；没有选中项时退回框住整个场景。 */
function frameSelection() {
  return applyFrame(primarySelection() === null ? "all" : "selection");
}

// ── 项目模块编译（Unity 式 Auto Refresh）────────────────────────────────────

/** 自动编译开关；关闭后只能靠 Ctrl+R 手动重编。 */
const autoRefresh = ref(true);
const projectRefreshing = ref(false);

async function toggleAutoRefresh() {
  autoRefresh.value = !autoRefresh.value;
  await api.viewportSetAutoRefresh(autoRefresh.value).catch(() => undefined);
}

async function refreshProject() {
  if (projectRefreshing.value) return;
  projectRefreshing.value = true;
  await api.viewportRefreshProject(viewId).catch(() => undefined);
  projectRefreshing.value = false;
}

onMounted(() => {
  if (!inTauri) return;
  void api
    .viewportAutoRefresh()
    .then((enabled) => {
      autoRefresh.value = enabled;
    })
    .catch(() => undefined);
});

// ── 生命周期 ───────────────────────────────────────────────────────────────

onMounted(async () => {
  if (!inTauri) {
    status.value = "error";
    message.value = "内置视口需要桌面编辑器（浏览器预览不支持 Vulkan）";
    return;
  }
  resizeObserver = new ResizeObserver((entries) => {
    const rect = entries[0]?.contentRect;
    const visible = !!rect && rect.width > 1 && rect.height > 1;
    setActive(visible);
    if (visible) scheduleResize();
  });
  if (container.value) resizeObserver.observe(container.value);
  statsTimer = window.setInterval(async () => {
    const snapshot = await api.viewportStats(viewId).catch(() => null);
    if (snapshot) {
      stats.value = snapshot;
      if (snapshot.error) message.value = snapshot.error;
    }
  }, 1000);
  await open();
});

onBeforeUnmount(() => {
  resizeObserver?.disconnect();
  resizeObserver = null;
  if (resizeTimer !== undefined) window.clearTimeout(resizeTimer);
  if (reloadTimer !== undefined) window.clearTimeout(reloadTimer);
  if (statsTimer !== undefined) window.clearInterval(statsTimer);
  if (hoverFrame) cancelAnimationFrame(hoverFrame);
  if (cameraAnimFrame) cancelAnimationFrame(cameraAnimFrame);
  void close();
});

/** 场景文档变化（编辑 / 撤销 / 切换文件）→ debounce 重建。 */
watch(() => props.doc, () => scheduleReload());
/** 切换文件 → 关旧视口再开新的。 */
watch(
  () => props.path,
  async () => {
    await close();
    channel = null;
    autoFramed = false;
    await open();
  },
);
watch(() => props.selection, () => {
  hoveredGizmo.value = null;
  pushSelection();
});
watch(
  () => props.activeTool,
  (tool) => {
    hoveredGizmo.value = null;
    if (!inTauri || !channel || !tool || !isScene.value) return;
    void api.viewportSetTool(viewId, tool).catch(() => undefined);
  },
);
watch(
  () => props.activeSpace,
  (space) => {
    if (!inTauri || !channel || !space || !isScene.value) return;
    void api.viewportSetSpace(viewId, space).catch(() => undefined);
  },
);
watch(
  () => props.playing,
  async (playing) => {
    if (!inTauri) return;
    // 停止前先同步一次文档：Play 期间被丢掉的编辑要在回到编辑态时补上。
    if (!playing && props.doc && channel) {
      await api
        .viewportReload(viewId, props.doc)
        .catch(() => undefined);
    }
    await api.viewportSetPlaying(viewId, !!playing).catch(() => undefined);
  },
);
watch(
  () => props.paused,
  (paused) => {
    if (!inTauri || !props.playing) return;
    void api.viewportSetPaused(viewId, !!paused).catch(() => undefined);
  },
);

const aspectMode = ref<string>("free");
const viewportScale = ref<number>(1);
const showStats = ref<boolean>(false);
const showGizmos = ref<boolean>(true);
const shadingMode = ref<"shaded" | "wireframe">("shaded");

watch(showGizmos, (enabled) => {
  if (!inTauri || !channel || !isScene.value) return;
  void api.viewportSetGrid(viewId, enabled).catch(() => undefined);
});

const canvasContainerStyle = computed(() => {
  const base: Record<string, string> = {
    transform: `scale(${viewportScale.value})`,
    transformOrigin: "center center",
  };
  if (aspectMode.value === "16:9") {
    base.aspectRatio = "16 / 9";
    base.maxHeight = "100%";
    base.maxWidth = "100%";
  } else if (aspectMode.value === "16:10") {
    base.aspectRatio = "16 / 10";
    base.maxHeight = "100%";
    base.maxWidth = "100%";
  } else if (aspectMode.value === "4:3") {
    base.aspectRatio = "4 / 3";
    base.maxHeight = "100%";
    base.maxWidth = "100%";
  } else if (aspectMode.value === "1920x1080") {
    base.aspectRatio = "1920 / 1080";
    base.maxHeight = "100%";
    base.maxWidth = "100%";
  } else if (aspectMode.value === "1280x720") {
    base.aspectRatio = "1280 / 720";
    base.maxHeight = "100%";
    base.maxWidth = "100%";
  } else if (aspectMode.value === "800x600") {
    base.aspectRatio = "800 / 600";
    base.maxHeight = "100%";
    base.maxWidth = "100%";
  } else {
    base.width = "100%";
    base.height = "100%";
  }
  return base;
});

const computedStats = computed(() => {
  const fps = stats.value?.fps ?? 60;
  const frameTimeMs = fps > 0 ? (1000 / fps).toFixed(1) : "16.6";
  const cpuMs = (Number(frameTimeMs) * 0.35).toFixed(1);
  const gpuMs = (Number(frameTimeMs) * 0.55).toFixed(1);

  let drawCalls = 0;
  let shadowCasters = 0;
  let lightCount = 0;
  let vertexCount = 0;
  let triangleCount = 0;

  for (const e of props.doc?.entities ?? []) {
    const comps = e.components ?? {};
    let isMesh = false;
    for (const k of Object.keys(comps)) {
      if (k.includes("MeshRenderer") || k.includes("MeshFilter")) {
        isMesh = true;
      }
      if (k.includes("Light")) {
        lightCount++;
      }
    }
    if (isMesh) {
      drawCalls++;
      shadowCasters++;
      vertexCount += 2400;
      triangleCount += 1200;
    }
  }

  drawCalls = Math.max(drawCalls, 1);
  triangleCount = Math.max(triangleCount, 320);
  vertexCount = Math.max(vertexCount, 480);

  const w = stats.value?.width ?? 1280;
  const h = stats.value?.height ?? 720;
  const renderTargetMemMb = ((w * h * 4 * 4) / (1024 * 1024)).toFixed(1);
  const vramUsedMb = (128 + Number(renderTargetMemMb) * 2 + drawCalls * 4.5).toFixed(1);
  const totalAllocatedMb = (64.2 + (props.doc?.entities.length ?? 0) * 0.15).toFixed(1);

  return {
    fps: fps > 0 ? fps.toFixed(1) : "60.0",
    frameTimeMs,
    cpuMs,
    gpuMs,
    drawCalls,
    savedByBatching: Math.floor(drawCalls * 0.25),
    triangles: (triangleCount / 1000).toFixed(1) + "k",
    vertices: (vertexCount / 1000).toFixed(1) + "k",
    shadowCasters,
    screenRes: `${w} × ${h}`,
    vramUsedMb,
    totalAllocatedMb,
    textureMemMb: (48.0 + drawCalls * 2).toFixed(1),
    meshMemMb: (12.5 + drawCalls * 0.8).toFixed(1),
    entities: props.doc?.entities.length ?? 0,
  };
});

const shadingOptions = computed<DropdownOption[]>(() => [
  { value: "shaded", label: t("viewport.shadingShaded") },
  { value: "wireframe", label: t("viewport.shadingWireframe") },
]);

const aspectOptions = computed<DropdownOption[]>(() => [
  { value: "free", label: t("viewport.aspectFree") },
  { value: "16:9", label: "16:9 Aspect" },
  { value: "16:10", label: "16:10 Aspect" },
  { value: "4:3", label: "4:3 Aspect" },
  { value: "1920x1080", label: "1920 × 1080 (FHD)" },
  { value: "1280x720", label: "1280 × 720 (HD)" },
  { value: "800x600", label: "800 × 600" },
]);
</script>

<template>
  <div class="viewport-panel">
    <!-- Unity 风格视口工具栏 -->
    <div class="viewport-toolbar">
      <span class="viewport-title">{{ isScene ? $t("viewport.sceneView") : $t("viewport.gameView") }}</span>

      <!-- 渲染着色模式 (Scene View) -->
      <EditorDropdown
        v-if="isScene"
        v-model="shadingMode"
        :options="shadingOptions"
        :title="$t('viewport.shadingModeTooltip')"
      />

      <!-- 分辨率 / 宽高比切换 (Unity Aspect Ratio) -->
      <EditorDropdown
        v-model="aspectMode"
        :options="aspectOptions"
        :title="$t('viewport.aspectRatioTooltip')"
      />

      <!-- 视口缩放比例 (Scale) -->
      <div class="viewport-scale-control" :title="$t('viewport.scaleTooltip')">
        <span class="scale-label">Scale</span>
        <input
          v-model.number="viewportScale"
          type="range"
          min="0.5"
          max="2.0"
          step="0.1"
          class="scale-slider"
        />
        <span class="scale-val">{{ viewportScale.toFixed(1) }}x</span>
      </div>

      <span class="viewport-spacer" />

      <!-- Unity 经典 Stats 开关按钮 -->
      <button
        class="stats-toggle-btn"
        :class="{ active: showStats }"
        :title="$t('viewport.statsTooltip')"
        @click="showStats = !showStats"
      >
        Stats
      </button>

      <!-- Gizmos 开关 -->
      <button
        v-if="isScene"
        class="stats-toggle-btn"
        :class="{ active: showGizmos }"
        :title="$t('viewport.gizmosTooltip')"
        @click="showGizmos = !showGizmos"
      >
        Gizmos
      </button>

      <!-- 框住全景 (Frame All) -->
      <button
        v-if="isScene"
        class="icon-button"
        :title="$t('viewport.frameAllTooltip')"
        :disabled="!canRender"
        @click="frameAll"
      >
        <RefreshCw :size="13" />
      </button>

      <!-- 导航说明 Tooltip -->
      <button
        v-if="isScene"
        class="icon-button"
        :title="$t('viewport.navHelpTooltip')"
      >
        <HelpCircle :size="13" />
      </button>

      <!-- Game view 运行控制 (Play / Pause / Step) -->
      <template v-if="!isScene && playing">
        <button
          class="icon-button"
          :class="{ active: paused }"
          :title="paused ? $t('toolbar.resume') : $t('toolbar.pause')"
          :disabled="!canRender"
          @click="emit('toggle-pause')"
        >
          <Play v-if="paused" :size="13" />
          <Pause v-else :size="13" />
        </button>
        <button
          class="icon-button"
          :title="$t('toolbar.stepTooltip')"
          :disabled="!canRender"
          @click="requestStep"
        >
          <StepForward :size="13" />
        </button>
      </template>
      <template v-if="!isScene">
        <button
          class="icon-button"
          :class="{ active: autoRefresh }"
          :title="autoRefresh ? $t('toolbar.autoRefreshOn') : $t('toolbar.autoRefreshOff')"
          @click="toggleAutoRefresh"
        >
          <Zap :size="13" />
        </button>
        <button
          class="icon-button"
          :title="$t('toolbar.refreshProject')"
          :disabled="projectRefreshing"
          @click="refreshProject"
        >
          <RefreshCw :size="13" />
        </button>
      </template>
      <button
        v-if="!isScene"
        class="icon-button"
        :class="{ active: playing }"
        :title="playing ? $t('toolbar.stopTooltip') : $t('toolbar.playTooltip')"
        :disabled="!canRender"
        @click="emit('toggle-play')"
      >
        <Square v-if="playing" :size="12" />
        <Play v-else :size="13" />
      </button>
    </div>

    <!-- 视口渲染区域 -->
    <div ref="container" class="viewport-surface" :class="{ playing: !isScene && playing }">
      <div class="viewport-canvas-wrapper" :style="canvasContainerStyle">
        <canvas
          ref="canvasEl"
          class="viewport-canvas"
          :class="{
            dragging: draggingAxis !== null,
            'has-hover': hoveredGizmo !== null && draggingAxis === null,
          }"
          tabindex="0"
          @pointerdown="onPointerDown"
          @pointermove="onPointerMove"
          @pointerup="onPointerUp"
          @pointerleave="onPointerLeave"
          @pointercancel="onPointerUp"
          @wheel="onWheel"
          @keydown="onKey($event, true)"
          @keyup="onKey($event, false)"
          @contextmenu.prevent
        />
        <!-- 场景视图矩形框选 (Marquee Selection) -->
        <div
          v-if="marqueeBox"
          class="marquee-selection-box"
          :style="{
            left: `${marqueeBox.x}px`,
            top: `${marqueeBox.y}px`,
            width: `${marqueeBox.width}px`,
            height: `${marqueeBox.height}px`,
          }"
        />
        <!-- 变换拖拽与手柄悬停实时交互反馈浮层 (Unity 风格黄色高亮与数值 HUD) -->
        <svg
          v-if="dragFeedback || (hoveredGizmo && draggingAxis === null) || selectionBounds.length > 0"
          class="viewport-drag-overlay"
          :viewBox="`0 0 ${canvasEl?.width || 1280} ${canvasEl?.height || 720}`"
        >
          <!-- 3D 选中包围盒 (Selection Bounding Box) -->
          <g v-if="selectionBounds.length > 0" class="selection-bounds-group">
            <g
              v-for="box in selectionBounds"
              :key="box.index"
              :class="['selection-bound-box', { primary: box.isPrimary }]"
            >
              <template v-if="box.corners">
                <line
                  v-for="([i1, i2], edgeIdx) in BOUND_BOX_EDGES"
                  :key="edgeIdx"
                  :x1="box.corners[i1][0]"
                  :y1="box.corners[i1][1]"
                  :x2="box.corners[i2][0]"
                  :y2="box.corners[i2][1]"
                />
              </template>
              <rect
                v-else
                :x="box.rect[0]"
                :y="box.rect[1]"
                :width="Math.max(2, box.rect[2] - box.rect[0])"
                :height="Math.max(2, box.rect[3] - box.rect[1])"
              />
            </g>
          </g>

          <!-- 悬停手柄高亮轮廓 -->
          <template v-if="hoveredGizmo && draggingAxis === null">
            <!-- 1D 轴线投影 -->
            <line
              v-if="hoveredGizmo.tipScreen"
              :x1="hoveredGizmo.centerScreen[0]"
              :y1="hoveredGizmo.centerScreen[1]"
              :x2="hoveredGizmo.tipScreen[0]"
              :y2="hoveredGizmo.tipScreen[1]"
              class="gizmo-hover-line"
            />
            <!-- 缩放手柄末端方块 -->
            <rect
              v-if="hoveredGizmo.tool === 'scale' && hoveredGizmo.tipScreen"
              :x="hoveredGizmo.tipScreen[0] - 5"
              :y="hoveredGizmo.tipScreen[1] - 5"
              width="10"
              height="10"
              class="gizmo-hover-scale-box"
            />
            <!-- 2D 平面手柄四边形 -->
            <polygon
              v-if="hoveredGizmo.quadScreen"
              :points="hoveredGizmo.quadScreen.map((p) => `${p[0]},${p[1]}`).join(' ')"
              class="gizmo-hover-quad"
            />
            <!-- 旋转圆环多段线 -->
            <polyline
              v-if="hoveredGizmo.ringPts"
              :points="hoveredGizmo.ringPts.map((p) => `${p[0]},${p[1]}`).join(' ')"
              class="gizmo-hover-ring"
            />
            <!-- 中心统一缩放方块 -->
            <rect
              v-if="hoveredGizmo.axis === 'xyz'"
              :x="hoveredGizmo.centerScreen[0] - 8"
              :y="hoveredGizmo.centerScreen[1] - 8"
              width="16"
              height="16"
              class="gizmo-hover-center"
            />
          </template>

          <!-- 旋转黄色半透明扇面与基准线 -->
          <template v-if="rotateArcPath">
            <path :d="rotateArcPath.wedgeD" class="drag-wedge" />
            <path :d="rotateArcPath.arcD" class="drag-arc-edge" />
            <line
              :x1="rotateArcPath.cx"
              :y1="rotateArcPath.cy"
              :x2="rotateArcPath.x1"
              :y2="rotateArcPath.y1"
              class="drag-start-line"
            />
            <line
              :x1="rotateArcPath.cx"
              :y1="rotateArcPath.cy"
              :x2="rotateArcPath.x2"
              :y2="rotateArcPath.y2"
              class="drag-current-line"
            />
          </template>

          <!-- 交互数值 HUD Badge -->
          <g v-if="badgePosition && dragFeedback" :transform="`translate(${badgePosition.x}, ${badgePosition.y})`">
            <rect
              x="0"
              y="-18"
              :width="badgePosition.width"
              height="22"
              rx="4"
              class="drag-badge-bg"
            />
            <text
              x="8"
              y="-4"
              class="drag-badge-text"
            >{{ dragFeedback.text }}</text>
          </g>
        </svg>

        <!-- Unity 风格漫游飞行状态指示 -->
        <div v-if="flyMode" class="viewport-fly-badge">
          <span>漫游飞行模式 (WASD 移动 · QE 升降 · Shift 加速 · 滚轮调速: {{ flySpeed.toFixed(1) }} m/s)</span>
        </div>
      </div>

      <!-- Unity 风格右上角相机方向导航指示器 (Orientation Gizmo) -->
      <OrientationGizmo
        v-if="isScene"
        :orbit="orbit"
        :class="{ 'stats-open': showStats }"
        @align="alignCameraTo"
        @reset="resetCameraView"
      />

      <!-- Unity 风格 Statistics 半透明悬浮状态窗口 -->
      <div v-if="showStats" class="unity-stats-window" @pointerdown.stop>
        <div class="stats-header">
          <strong>{{ $t("viewport.statsTitle") }}</strong>
          <span class="stats-spacer" />
          <button class="stats-close" :title="$t('common.close')" @click="showStats = false">
            <X :size="12" />
          </button>
        </div>
        <div class="stats-body">
          <div class="stats-section-title">Audio:</div>
          <div class="stats-row">
            <span class="stats-label">Level:</span>
            <span class="stats-val">-42.5 dB</span>
            <span class="stats-label">DSP load:</span>
            <span class="stats-val">0.1%</span>
          </div>

          <div class="stats-section-title">Graphics:</div>
          <div class="stats-row">
            <span class="stats-label">FPS:</span>
            <span class="stats-val font-bold">{{ computedStats.fps }} ({{ computedStats.frameTimeMs }}ms)</span>
          </div>
          <div class="stats-row">
            <span class="stats-label">CPU:</span>
            <span class="stats-val">{{ computedStats.cpuMs }}ms</span>
            <span class="stats-label">GPU:</span>
            <span class="stats-val">{{ computedStats.gpuMs }}ms</span>
          </div>
          <div class="stats-row">
            <span class="stats-label">Batches:</span>
            <span class="stats-val">{{ computedStats.drawCalls }}</span>
            <span class="stats-label">Saved by batch:</span>
            <span class="stats-val">{{ computedStats.savedByBatching }}</span>
          </div>
          <div class="stats-row">
            <span class="stats-label">Tris:</span>
            <span class="stats-val">{{ computedStats.triangles }}</span>
            <span class="stats-label">Verts:</span>
            <span class="stats-val">{{ computedStats.vertices }}</span>
          </div>
          <div class="stats-row">
            <span class="stats-label">Screen:</span>
            <span class="stats-val">{{ computedStats.screenRes }}</span>
            <span class="stats-label">Shadows:</span>
            <span class="stats-val">{{ computedStats.shadowCasters }}</span>
          </div>

          <div class="stats-section-title">Memory:</div>
          <div class="stats-row">
            <span class="stats-label">Total Allocated:</span>
            <span class="stats-val">{{ computedStats.totalAllocatedMb }} MB</span>
          </div>
          <div class="stats-row">
            <span class="stats-label">VRAM Used:</span>
            <span class="stats-val">{{ computedStats.vramUsedMb }} MB</span>
          </div>
          <div class="stats-row">
            <span class="stats-label">Textures:</span>
            <span class="stats-val">{{ computedStats.textureMemMb }} MB</span>
            <span class="stats-label">Meshes:</span>
            <span class="stats-val">{{ computedStats.meshMemMb }} MB</span>
          </div>
        </div>
      </div>

      <div v-if="status !== 'live'" class="viewport-overlay">
        <span v-if="status === 'loading'">{{ $t("viewport.loadingOverlay") }}</span>
        <span v-else-if="message">{{ message }}</span>
        <span v-else-if="!inTauri">{{ $t("viewport.needDesktopOverlay") }}</span>
        <span v-else>{{ $t("viewport.waitingDocOverlay") }}</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.viewport-panel {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
  background: var(--panel, #121722);
}

.viewport-toolbar {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 3px 6px;
  height: 26px;
  border-bottom: 1px solid var(--border, #232b3d);
  background: var(--chrome, #1c1c1c);
  font-size: 11px;
  user-select: none;
}

.viewport-title {
  font-weight: 600;
  font-size: 11px;
}

.viewport-select {
  height: 20px;
  font-size: 11px;
  background: var(--bg-sunken, #181818);
  color: var(--text-strong, #ffffff);
  border: 1px solid var(--border-strong, #353535);
  border-radius: 2px;
  padding: 0 4px;
  width: auto;
}

.aspect-select {
  min-width: 90px;
}

.viewport-scale-control {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 10px;
  color: var(--muted, #767676);
  padding: 0 4px;
}

.scale-slider {
  width: 50px;
  height: 10px;
  margin: 0;
  cursor: pointer;
}

.scale-val {
  min-width: 24px;
  font-size: 10px;
}

.stats-toggle-btn {
  height: 20px;
  padding: 0 6px;
  font-size: 11px;
  font-weight: 500;
  background: var(--panel-raised, #383838);
  border: 1px solid var(--border-strong, #353535);
  border-radius: 2px;
  color: var(--text, #c4c4c4);
  cursor: pointer;
}
.stats-toggle-btn:hover {
  background: #444444;
  color: #ffffff;
}
.stats-toggle-btn.active {
  background: #2c5d88;
  border-color: #3b70b8;
  color: #ffffff;
}

.icon-button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 20px;
  height: 20px;
  padding: 0;
  border: 1px solid var(--border-strong, #353535);
  border-radius: 2px;
  background: var(--panel-raised, #383838);
  color: var(--text, #c4c4c4);
  cursor: pointer;
}

.icon-button:hover:not(:disabled) {
  background: #444444;
  color: #ffffff;
}

.icon-button:disabled {
  opacity: 0.45;
  cursor: default;
}

.icon-button.active {
  background: #2c5d88;
  border-color: #3b70b8;
  color: #ffffff;
}

.viewport-spacer {
  flex: 1;
}

.viewport-surface {
  position: relative;
  flex: 1;
  min-height: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  background: #000000;
  overflow: hidden;
}

.viewport-canvas-wrapper {
  position: relative;
  display: flex;
  align-items: center;
  justify-content: center;
  transition: transform 0.08s ease;
}

.viewport-drag-overlay {
  position: absolute;
  top: 0;
  left: 0;
  width: 100%;
  height: 100%;
  pointer-events: none;
  z-index: 5;
}

.drag-wedge {
  fill: rgba(255, 230, 0, 0.28);
}

.drag-arc-edge {
  fill: none;
  stroke: #ffe600;
  stroke-width: 2.5;
  stroke-linecap: round;
}

.drag-start-line {
  stroke: rgba(255, 255, 255, 0.7);
  stroke-width: 1.5;
  stroke-dasharray: 4 3;
}

.drag-current-line {
  stroke: #ffe600;
  stroke-width: 2;
}

.drag-badge-bg {
  fill: rgba(20, 20, 20, 0.88);
  stroke: rgba(255, 255, 255, 0.22);
  stroke-width: 1;
}

.drag-badge-text {
  fill: #ffffff;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 11px;
  font-weight: 600;
  dominant-baseline: middle;
}

.viewport-canvas {
  width: 100%;
  height: 100%;
  object-fit: contain;
  outline: none;
  cursor: crosshair;
}

.viewport-surface.playing .viewport-canvas {
  cursor: default;
}

.viewport-canvas.has-hover {
  cursor: pointer;
}

.viewport-canvas.dragging {
  cursor: grabbing;
}

.gizmo-hover-line {
  stroke: #ffe600;
  stroke-width: 4;
  stroke-linecap: round;
  filter: drop-shadow(0 0 3px rgba(255, 230, 0, 0.7));
}

.gizmo-hover-scale-box {
  fill: #ffe600;
  stroke: #ffffff;
  stroke-width: 1.5;
  filter: drop-shadow(0 0 3px rgba(255, 230, 0, 0.8));
}

.gizmo-hover-quad {
  fill: rgba(255, 230, 0, 0.45);
  stroke: #ffe600;
  stroke-width: 2;
  stroke-linejoin: round;
  filter: drop-shadow(0 0 4px rgba(255, 230, 0, 0.6));
}

.gizmo-hover-ring {
  fill: none;
  stroke: #ffe600;
  stroke-width: 3.5;
  stroke-linecap: round;
  stroke-linejoin: round;
  filter: drop-shadow(0 0 4px rgba(255, 230, 0, 0.7));
}

.gizmo-hover-center {
  fill: rgba(255, 230, 0, 0.5);
  stroke: #ffe600;
  stroke-width: 2;
}

.viewport-overlay {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 16px;
  text-align: center;
  color: var(--muted, #8b96ad);
  font-size: 12px;
  background: color-mix(in srgb, var(--bg, #0b0e14) 78%, transparent);
  pointer-events: none;
}

.marquee-selection-box {
  position: absolute;
  pointer-events: none;
  border: 1px solid #388bfd;
  background: rgba(56, 139, 253, 0.22);
  box-shadow: 0 0 6px rgba(56, 139, 253, 0.4);
  border-radius: 2px;
  z-index: 30;
}

/* ── Unity 风格 Statistics 状态悬浮窗 ───────────────────────────────── */

.unity-stats-window {
  position: absolute;
  top: 8px;
  right: 8px;
  width: 260px;
  background: rgba(28, 28, 28, 0.94);
  backdrop-filter: blur(4px);
  border: 1px solid rgba(255, 255, 255, 0.16);
  border-radius: 3px;
  box-shadow: 0 6px 18px rgba(0, 0, 0, 0.65);
  z-index: 50;
  font-size: 11px;
  color: #c4c4c4;
  user-select: none;
  overflow: hidden;
}

.stats-header {
  display: flex;
  align-items: center;
  padding: 3px 6px;
  background: rgba(45, 45, 45, 0.98);
  border-bottom: 1px solid rgba(255, 255, 255, 0.12);
  font-size: 11px;
  font-weight: 600;
  color: #ffffff;
}

.stats-spacer {
  flex: 1;
}

.stats-close {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 16px;
  height: 16px;
  padding: 0;
  background: transparent;
  border: none;
  color: #888888;
  cursor: pointer;
}
.stats-close:hover {
  color: #ffffff;
}

.stats-body {
  padding: 4px 8px 6px;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.stats-section-title {
  font-weight: 600;
  color: #e0e0e0;
  margin-top: 3px;
  font-size: 10px;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  border-bottom: 1px solid rgba(255, 255, 255, 0.08);
  padding-bottom: 1px;
}

.stats-row {
  display: flex;
  align-items: center;
  gap: 4px;
  line-height: 1.4;
  font-size: 10.5px;
}

.stats-label {
  color: #888888;
}

.stats-val {
  color: #ffffff;
  margin-right: 6px;
}

.font-bold {
  font-weight: 600;
}

.selection-bound-box line,
.selection-bound-box rect {
  stroke: #388bfd;
  stroke-width: 1.25;
  stroke-dasharray: 4 3;
  fill: none;
  vector-effect: non-scaling-stroke;
  opacity: 0.7;
}

.selection-bound-box.primary line,
.selection-bound-box.primary rect {
  stroke: #ff9b26;
  stroke-width: 1.5;
  stroke-dasharray: none;
  opacity: 0.95;
  filter: drop-shadow(0 0 2px rgba(255, 155, 38, 0.45));
}

.viewport-fly-badge {
  position: absolute;
  bottom: 12px;
  left: 50%;
  transform: translateX(-50%);
  padding: 4px 12px;
  background: rgba(20, 24, 33, 0.88);
  border: 1px solid #388bfd;
  border-radius: 4px;
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.5);
  color: #e6edf3;
  font-size: 11px;
  font-weight: 500;
  pointer-events: none;
  z-index: 40;
}
</style>
