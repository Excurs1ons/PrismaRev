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
import { Pause, Play, RefreshCw, Square, StepForward } from "lucide-vue-next";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import * as api from "../api";
import type { CameraOrbit, SceneDoc, ViewportInput, ViewportSnapshot } from "../types";

const props = defineProps<{
  kind: "scene" | "game";
  doc: SceneDoc | null;
  path: string | null;
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
  /** 拖动平移手柄后的新局部位置 `[x, y, z]`。 */
  (event: "move", position: [number, number, number]): void;
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
    message.value = "内置视口需要桌面编辑器（浏览器预览不支持 Vulkan）";
    status.value = "error";
    return;
  }
  if (!props.doc || !props.path) {
    status.value = "idle";
    return;
  }
  status.value = "loading";
  message.value = null;
  const { width, height } = physicalSize();
  channel = new Channel<ArrayBuffer>();
  channel.onmessage = (buffer) => drawFrame(buffer);
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
    // 第一帧到达时 drawFrame 才切到 live：渲染器还在建管线时先保留 loading 浮层，
    // 否则会先闪一段黑屏。
  } catch (error) {
    status.value = "error";
    message.value = String(error);
  }
}

async function close() {
  if (!inTauri) return;
  channel = null;
  frames.value = 0;
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
  }
  const context = canvas.getContext("2d");
  if (!context) return;
  const pixels = new Uint8ClampedArray(buffer, 8);
  context.putImageData(new ImageData(pixels, width, height), 0, 0);
  frames.value += 1;
  status.value = "live";
  if (isScene.value && !autoFramed) {
    autoFramed = true;
    void frameAll();
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
      message.value = null;
    } catch (error) {
      message.value = String(error);
      status.value = "error";
    }
  }, 450);
}

// ── 相机 / 输入 ────────────────────────────────────────────────────────────

let dragMode: "orbit" | "pan" | null = null;
let lastPointer = { x: 0, y: 0 };
/** 判定「点选」还是「拖动相机」：按下位置 + 时间。 */
let clickStart: { x: number; y: number; at: number } | null = null;
/** 正在拖动的平移手柄（`"x"` / `"y"` / `"z"`），`null` = 没在拖。 */
const draggingAxis = ref<string | null>(null);
/** 拖动期间待下发的光标位置 / 是否攒了未同步的文档重载。 */
let pendingDrag: { x: number; y: number } | null = null;
let reloadPending = false;
let dragFrame = 0;
/** 左键按下瞬间的相机（万一这一按其实落在手柄上，用于还原）。 */
let orbitBeforePress: CameraOrbit | null = null;

/** 相机变更按 rAF 合并：拖动时每帧最多下发一次。 */
function pushCamera() {
  if (!inTauri || !isScene.value) return;
  if (cameraPending) return;
  cameraPending = true;
  requestAnimationFrame(() => {
    cameraPending = false;
    void api.viewportCamera(viewId, orbit.value).catch(() => undefined);
  });
}

function onPointerDown(event: PointerEvent) {
  const canvas = canvasEl.value;
  if (!canvas) return;
  canvas.focus();
  if (isScene.value) {
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
  draggingAxis.value = axis;
  canvas.setPointerCapture(event.pointerId);
  event.preventDefault();
}

function onPointerMove(event: PointerEvent) {
  if (isScene.value) {
    if (draggingAxis.value) {
      const point = canvasPoint(event);
      if (point) pushDrag(point);
      return;
    }
    if (!dragMode) return;
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
function pushDrag(point: { x: number; y: number }) {
  if (dragFrame) {
    pendingDrag = point;
    return;
  }
  dragFrame = requestAnimationFrame(() => {
    dragFrame = 0;
    const target = pendingDrag ?? point;
    pendingDrag = null;
    void api
      .viewportDrag(viewId, target.x, target.y)
      .then((position) => {
        if (!position) return;
        emit("move", position);
        // 拖动期间文档在变，但重载会换掉 ECS 句柄：等松手再同步。
        reloadPending = true;
      })
      .catch(() => undefined);
  });
}

function onPointerUp(event: PointerEvent) {
  orbitBeforePress = null;
  if (isScene.value) {
    if (draggingAxis.value) {
      draggingAxis.value = null;
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

/** 点选：把 CSS 坐标换算到 canvas **位图**坐标再交给后端。 */
async function pickAt(event: PointerEvent) {
  if (!inTauri || !channel) return;
  const point = canvasPoint(event);
  if (!point) return;
  const hit = await api.viewportPick(viewId, point.x, point.y).catch(() => undefined);
  if (hit === undefined) return;
  emit("select", hit ?? null);
}

/** 主选中项：多选时与层级一样取最后一个。 */
function primarySelection(): number | null {
  const selection = props.selection;
  return selection && selection.length ? selection[selection.length - 1] : null;
}

/** 把选中项同步给后端（决定 gizmo 画在哪个实体上）。 */
function pushSelection() {
  if (!inTauri || !channel || !isScene.value) return;
  void api.viewportSelect(viewId, primarySelection()).catch(() => undefined);
}

function onWheel(event: WheelEvent) {
  if (isScene.value) {
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

/** 套用后端算出来的取景参数（`yaw` / `pitch` 由后端原样带回）。 */
async function applyFrame(mode: "all" | "selection") {
  if (!inTauri || !channel) return;
  const framed = await api.viewportFrame(viewId, mode).catch(() => null);
  if (!framed) return;
  orbit.value = framed;
  pushCamera();
}

/** 框住整个场景（`A` / `Home` / 工具栏按钮）。 */
function frameAll() {
  return applyFrame("all");
}

/** 聚焦选中项；没有选中项时退回框住整个场景。 */
function frameSelection() {
  return applyFrame(primarySelection() === null ? "all" : "selection");
}

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
watch(() => props.selection, pushSelection);
watch(
  () => props.playing,
  (playing) => {
    if (!inTauri) return;
    void api.viewportSetPlaying(viewId, !!playing).catch(() => undefined);
  },
);
watch(
  () => props.paused,
  (paused) => {
    if (!inTauri || !props.playing) return;
    void api.viewportSetPaused(viewId, !!paused).catch(() => undefined);
  },
);

const fpsLabel = computed(() => {
  const fps = stats.value?.fps ?? 0;
  return fps > 0 ? `${fps.toFixed(0)} fps` : "—";
});

const entityLabel = computed(() =>
  stats.value ? `${stats.value.entities} 实体` : "",
);
</script>

<template>
  <div class="viewport-panel">
    <div class="viewport-toolbar">
      <span class="viewport-title">{{ isScene ? "场景视图" : "游戏视图" }}</span>
      <span class="muted viewport-meta">{{ fpsLabel }}</span>
      <span v-if="entityLabel" class="muted viewport-meta">{{ entityLabel }}</span>
      <span class="viewport-spacer" />
      <button
        v-if="isScene"
        class="icon-button"
        title="框住场景（A）"
        :disabled="!canRender"
        @click="frameAll"
      >
        <RefreshCw :size="13" />
      </button>
      <button
        v-if="!isScene && playing"
        class="icon-button"
        :class="{ active: paused }"
        :title="paused ? '继续' : '暂停'"
        :disabled="!canRender"
        @click="emit('toggle-pause')"
      >
        <Play v-if="paused" :size="13" />
        <Pause v-else :size="13" />
      </button>
      <button
        v-if="!isScene && playing"
        class="icon-button"
        title="单步（前进一帧）"
        :disabled="!canRender"
        @click="requestStep"
      >
        <StepForward :size="13" />
      </button>
      <button
        v-if="!isScene"
        class="icon-button"
        :class="{ active: playing }"
        :title="playing ? '停止' : '运行'"
        :disabled="!canRender"
        @click="emit('toggle-play')"
      >
        <Square v-if="playing" :size="12" />
        <Play v-else :size="13" />
      </button>
    </div>

    <div ref="container" class="viewport-surface" :class="{ playing: !isScene && playing }">
      <canvas
        ref="canvasEl"
        class="viewport-canvas"
        :class="{ dragging: draggingAxis !== null }"
        tabindex="0"
        @pointerdown="onPointerDown"
        @pointermove="onPointerMove"
        @pointerup="onPointerUp"
        @pointercancel="onPointerUp"
        @wheel="onWheel"
        @keydown="onKey($event, true)"
        @keyup="onKey($event, false)"
        @contextmenu.prevent
      />
      <div v-if="status !== 'live'" class="viewport-overlay">
        <span v-if="status === 'loading'">编译管线 / 加载场景…</span>
        <span v-else-if="message">{{ message }}</span>
        <span v-else-if="!inTauri">内置视口需要桌面编辑器</span>
        <span v-else>等待场景文档…</span>
      </div>
      <div v-if="isScene" class="viewport-hint">
        拖动坐标轴移动 · F 聚焦选中 · A 框住场景 · 左键轨道 · 中键/Shift 平移 · 滚轮缩放
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
  gap: 8px;
  padding: 4px 8px;
  border-bottom: 1px solid var(--border, #232b3d);
  font-size: 12px;
}

.viewport-title {
  font-weight: 600;
}

.icon-button {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 22px;
  height: 22px;
  padding: 0;
  border: 1px solid var(--border, #232b3d);
  border-radius: 4px;
  background: var(--panel-alt, #171d2b);
  color: var(--muted, #8b96ad);
  cursor: pointer;
}

.icon-button:hover:not(:disabled) {
  border-color: var(--border-strong, #333843);
  color: var(--text, #e6ebf5);
}

.icon-button:disabled {
  opacity: 0.45;
  cursor: default;
}

.icon-button.active {
  border-color: var(--accent, #6ea8ff);
  color: var(--accent, #6ea8ff);
}

.viewport-meta {
  font-size: 11px;
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
  background: var(--bg-sunken, #0d1119);
  overflow: hidden;
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

.viewport-canvas.dragging {
  cursor: grabbing;
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

.viewport-hint {
  position: absolute;
  left: 8px;
  bottom: 6px;
  font-size: 11px;
  color: var(--muted, #8b96ad);
  opacity: 0.75;
  pointer-events: none;
}
</style>
