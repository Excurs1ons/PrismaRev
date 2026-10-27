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
import { Pause, Play, RefreshCw } from "lucide-vue-next";
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import * as api from "../api";
import type { CameraOrbit, SceneDoc, ViewportInput, ViewportSnapshot } from "../types";

const props = defineProps<{
  kind: "scene" | "game";
  doc: SceneDoc | null;
  path: string | null;
  /** Game view 的 Play 状态（由工具栏统一控制）。 */
  playing?: boolean;
}>();

const emit = defineEmits<{ (event: "toggle-play"): void }>();

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
    if (event.button === 1 || event.shiftKey) {
      dragMode = "pan";
    } else if (event.button === 0 || event.button === 2) {
      dragMode = "orbit";
    } else {
      return;
    }
    lastPointer = { x: event.clientX, y: event.clientY };
    canvas.setPointerCapture(event.pointerId);
    event.preventDefault();
    return;
  }
  void sendInput({ type: "mouseButton", button: event.button, pressed: true });
}

function onPointerMove(event: PointerEvent) {
  if (isScene.value) {
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

function onPointerUp(event: PointerEvent) {
  if (isScene.value) {
    dragMode = null;
    canvasEl.value?.releasePointerCapture(event.pointerId);
    return;
  }
  void sendInput({ type: "mouseButton", button: event.button, pressed: false });
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
  if (isScene.value) return;
  void sendInput({ type: "key", code: event.code, pressed });
  event.preventDefault();
}

function sendInput(event: ViewportInput) {
  if (!inTauri || !channel) return Promise.resolve();
  return api.viewportInput(viewId, event).catch(() => undefined);
}

function resetCamera() {
  orbit.value = { target: [0, 0, 0], yaw: -0.7, pitch: 0.35, distance: 12 };
  pushCamera();
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
    await open();
  },
);
watch(
  () => props.playing,
  (playing) => {
    if (!inTauri) return;
    void api.viewportSetPlaying(viewId, !!playing).catch(() => undefined);
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
        title="重置相机"
        :disabled="!canRender"
        @click="resetCamera"
      >
        <RefreshCw :size="13" />
      </button>
      <button
        v-if="!isScene"
        class="icon-button"
        :title="playing ? '停止' : '运行'"
        :disabled="!canRender"
        @click="emit('toggle-play')"
      >
        <Pause v-if="playing" :size="13" />
        <Play v-else :size="13" />
      </button>
    </div>

    <div ref="container" class="viewport-surface" :class="{ playing: !isScene && playing }">
      <canvas
        ref="canvasEl"
        class="viewport-canvas"
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
        左键轨道 · 中键/Shift 平移 · 滚轮缩放
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
  background: var(--panel-bg, #15171c);
}

.viewport-toolbar {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 4px 8px;
  border-bottom: 1px solid var(--border, #262a33);
  font-size: 12px;
}

.viewport-title {
  font-weight: 600;
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
  background: #0b0c0f;
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

.viewport-overlay {
  position: absolute;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 16px;
  text-align: center;
  color: var(--text-muted, #8b93a1);
  font-size: 12px;
  background: rgba(8, 9, 12, 0.72);
  pointer-events: none;
}

.viewport-hint {
  position: absolute;
  left: 8px;
  bottom: 6px;
  font-size: 11px;
  color: var(--text-muted, #8b93a1);
  opacity: 0.75;
  pointer-events: none;
}
</style>
