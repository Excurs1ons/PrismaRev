<script setup lang="ts">
import { computed, onMounted, onUnmounted, provide, reactive, ref } from "vue";
import { X } from "lucide-vue-next";
import DockNode from "./DockNode.vue";
import {
  activateTab,
  clampFloatPanel,
  dockPanelAtEdge,
  insertPanel,
  leaf,
  removePanel,
  setRatio,
  MIN_FLOAT_HEIGHT,
  MIN_FLOAT_WIDTH,
  type DockEdge,
  type DockNode as DockNodeType,
  type DockZone,
  type FloatPanel,
} from "../dock";
import { DOCK_KEY, type DockHover } from "../dockContext";

const props = defineProps<{
  panels: Array<{ id: string; title: string }>;
}>();

const layout = defineModel<DockNodeType | null>("layout", { required: true });
const floating = defineModel<FloatPanel[]>("floating", { default: () => [] });

const rootEl = ref<HTMLElement | null>(null);
const dragPanel = ref<string | null>(null);
const hover = ref<DockHover | null>(null);
const edgeHover = ref<DockEdge | null>(null);
const dragPos = reactive({ x: 0, y: 0 });
/** 只允许一个拖拽会话，避免旧 pointer 监听器在新拖拽的 pointerup 上重复触发。 */
let dragSeq = 0;

const titles = computed(() => new Map(props.panels.map((panel) => [panel.id, panel.title])));
const floatingSorted = computed(() => [...floating.value].sort((a, b) => a.z - b.z));

function titleOf(id: string): string {
  return titles.value.get(id) ?? id;
}

function nextZ(): number {
  return floating.value.reduce((max, panel) => Math.max(max, panel.z), 0) + 1;
}

/** 点击/拖动浮动面板时置顶。 */
function focus(tab: string) {
  const panel = floating.value.find((item) => item.tab === tab);
  if (!panel) return;
  const z = nextZ();
  if (panel.z === z) return;
  floating.value = floating.value.map((item) => (item.tab === tab ? { ...item, z } : item));
}

function select(leafId: string, tab: string) {
  if (!layout.value) return;
  layout.value = activateTab(layout.value, leafId, tab);
}

function close(tab: string) {
  if (layout.value) layout.value = removePanel(layout.value, tab);
  floating.value = floating.value.filter((panel) => panel.tab !== tab);
}

function resize(splitId: string, ratio: number) {
  if (!layout.value) return;
  layout.value = setRatio(layout.value, splitId, ratio);
}

function drop(leafId: string, zone: DockZone, tab: string) {
  floating.value = floating.value.filter((panel) => panel.tab !== tab);
  if (!layout.value) {
    layout.value = leaf([tab]);
    dragPanel.value = null;
    hover.value = null;
    return;
  }
  layout.value = insertPanel(layout.value, leafId, tab, zone);
  dragPanel.value = null;
  hover.value = null;
}

function zoneAt(node: HTMLElement, x: number, y: number): DockZone {
  const rect = node.getBoundingClientRect();
  const px = (x - rect.left) / Math.max(1, rect.width);
  const py = (y - rect.top) / Math.max(1, rect.height);
  const edge = 0.25;
  if (px < edge) return "left";
  if (px > 1 - edge) return "right";
  if (py < edge) return "top";
  if (py > 1 - edge) return "bottom";
  return "center";
}

/**
 * 找到指针下方的 dock leaf。
 *
 * 用 `elementsFromPoint` 而不是 `elementFromPoint`：拖动浮动窗口时，
 * 窗口本身在最上层，但命中的叶子仍应能被识别出来用于重新停靠。
 */
function leafUnder(x: number, y: number): HTMLElement | null {
  for (const el of document.elementsFromPoint(x, y)) {
    const leafEl = (el as HTMLElement).closest?.<HTMLElement>(".dock-leaf[data-leaf-id]");
    if (leafEl?.dataset.leafId) return leafEl;
  }
  return null;
}

/** 主窗口边缘（左/右/底）优先作为停靠目标。 */
const EDGE_PX = 28;

function edgeAt(x: number, y: number): DockEdge | null {
  const el = rootEl.value;
  if (!el) return null;
  const rect = el.getBoundingClientRect();
  if (x < rect.left || x > rect.right || y < rect.top || y > rect.bottom) return null;
  if (x - rect.left <= EDGE_PX) return "left";
  if (rect.right - x <= EDGE_PX) return "right";
  if (rect.bottom - y <= EDGE_PX) return "bottom";
  return null;
}

function updateHover(x: number, y: number) {
  const edge = edgeAt(x, y);
  if (edge) {
    edgeHover.value = edge;
    hover.value = null;
    return;
  }
  edgeHover.value = null;

  const leafEl = leafUnder(x, y);
  if (!leafEl?.dataset.leafId) {
    hover.value = null;
    return;
  }
  hover.value = { leafId: leafEl.dataset.leafId, zone: zoneAt(leafEl, x, y) };
}

/** 停靠到主窗口左/右/底边（整棵树的外侧）。 */
function dockEdge(tab: string, edge: DockEdge) {
  floating.value = floating.value.filter((panel) => panel.tab !== tab);
  layout.value = dockPanelAtEdge(layout.value, tab, edge);
  dragPanel.value = null;
  hover.value = null;
  edgeHover.value = null;
}

/** 指针释放在停靠区外：从 dock tree 摘下来变成自由浮动窗口。 */
function detach(tab: string, x: number, y: number) {
  if (layout.value) layout.value = removePanel(layout.value, tab);
  floating.value = floating.value.filter((panel) => panel.tab !== tab);
  const width = 300;
  const height = 340;
  floating.value = [
    ...floating.value,
    clampFloatPanel(
      { tab, x: x - width / 2, y: y - 22, width, height, z: nextZ() },
      window.innerWidth,
      window.innerHeight,
    ),
  ];
}

/**
 * 标签页指针拖拽。
 *
 * 不使用 HTML5 drag-and-drop：Tauri/WebView2 会接管原生拖放，
 * 且不同平台的 drag 事件时序不稳定。这里直接跟踪 pointer，
 * 用 `elementsFromPoint` 找停靠目标，行为与 Unity/ImGui 一致。
 */
function startDrag(tab: string, event: PointerEvent) {
  if (event.button !== 0) return;
  const seq = ++dragSeq;
  const capture = event.currentTarget as HTMLElement | null;
  capture?.setPointerCapture?.(event.pointerId);
  const startX = event.clientX;
  const startY = event.clientY;
  let started = false;
  let forceFloat = false;

  const move = (ev: PointerEvent) => {
    if (seq !== dragSeq) return;
    dragPos.x = ev.clientX;
    dragPos.y = ev.clientY;
    forceFloat = ev.altKey;

    if (!started) {
      if (Math.hypot(ev.clientX - startX, ev.clientY - startY) < 5) return;
      started = true;
      dragPanel.value = tab;
      document.body.style.userSelect = "none";
      document.body.style.cursor = "grabbing";
    }
    if (forceFloat) {
      hover.value = null;
      edgeHover.value = null;
    } else {
      updateHover(ev.clientX, ev.clientY);
    }
  };

  const finish = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", finish);
    window.removeEventListener("pointercancel", finish);
    if (seq !== dragSeq) return;
    document.body.style.userSelect = "";
    document.body.style.cursor = "";

    if (started) {
      const edge = edgeHover.value;
      const target = hover.value;
      if (edge && !forceFloat) dockEdge(tab, edge);
      else if (target && !forceFloat) drop(target.leafId, target.zone, tab);
      else detach(tab, dragPos.x, dragPos.y);
    }
    dragPanel.value = null;
    hover.value = null;
    edgeHover.value = null;
  };

  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", finish);
  window.addEventListener("pointercancel", finish);
}

/** 拖动浮动窗口标题栏；放到 dock 区域会重新停靠。 */
function startFloatDrag(tab: string, event: PointerEvent) {
  if (event.button !== 0) return;
  const seq = ++dragSeq;
  focus(tab);
  const panel = floating.value.find((item) => item.tab === tab);
  if (!panel) return;
  const capture = event.currentTarget as HTMLElement | null;
  capture?.setPointerCapture?.(event.pointerId);
  const startX = event.clientX;
  const startY = event.clientY;
  const originX = panel.x;
  const originY = panel.y;
  let forceFloat = false;

  const move = (ev: PointerEvent) => {
    if (seq !== dragSeq) return;
    forceFloat = ev.altKey;
    if (forceFloat) {
      hover.value = null;
      edgeHover.value = null;
    } else {
      updateHover(ev.clientX, ev.clientY);
    }
    const dx = ev.clientX - startX;
    const dy = ev.clientY - startY;
    floating.value = floating.value.map((item) =>
      item.tab === tab ? { ...item, x: originX + dx, y: originY + dy } : item,
    );
  };

  const finish = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", finish);
    window.removeEventListener("pointercancel", finish);
    if (seq !== dragSeq) return;
    document.body.style.userSelect = "";
    document.body.style.cursor = "";

    const edge = edgeHover.value;
    const target = hover.value;
    if (edge && !forceFloat) {
      dockEdge(tab, edge);
    } else if (target && !forceFloat) {
      drop(target.leafId, target.zone, tab);
    } else {
      floating.value = floating.value.map((item) =>
        item.tab === tab ? clampFloatPanel(item, window.innerWidth, window.innerHeight) : item,
      );
    }
    hover.value = null;
    edgeHover.value = null;
  };

  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", finish);
  window.addEventListener("pointercancel", finish);
}

/** 拖动右下角调整浮动窗口尺寸。 */
function startFloatResize(tab: string, event: PointerEvent) {
  if (event.button !== 0) return;
  const seq = ++dragSeq;
  event.preventDefault();
  focus(tab);
  const panel = floating.value.find((item) => item.tab === tab);
  if (!panel) return;
  const capture = event.currentTarget as HTMLElement | null;
  capture?.setPointerCapture?.(event.pointerId);
  const startX = event.clientX;
  const startY = event.clientY;
  const originW = panel.width;
  const originH = panel.height;

  const move = (ev: PointerEvent) => {
    if (seq !== dragSeq) return;
    const width = Math.max(MIN_FLOAT_WIDTH, originW + ev.clientX - startX);
    const height = Math.max(MIN_FLOAT_HEIGHT, originH + ev.clientY - startY);
    floating.value = floating.value.map((item) =>
      item.tab === tab ? { ...item, width, height } : item,
    );
  };

  const finish = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", finish);
    window.removeEventListener("pointercancel", finish);
    if (seq !== dragSeq) return;
    floating.value = floating.value.map((item) =>
      item.tab === tab ? clampFloatPanel(item, window.innerWidth, window.innerHeight) : item,
    );
  };

  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", finish);
  window.addEventListener("pointercancel", finish);
}

function clampAllFloating() {
  if (!floating.value.length) return;
  floating.value = floating.value.map((panel) =>
    clampFloatPanel(panel, window.innerWidth, window.innerHeight),
  );
}

onMounted(() => window.addEventListener("resize", clampAllFloating));
onUnmounted(() => window.removeEventListener("resize", clampAllFloating));

provide(DOCK_KEY, {
  dragPanel,
  hover,
  titleOf,
  select,
  close,
  resize,
  drop,
  startDrag,
});
</script>

<template>
  <div ref="rootEl" class="dock-root">
    <DockNode v-if="layout" :node="layout">
      <template v-for="(_, name) in $slots" #[name]>
        <slot :name="name" />
      </template>
    </DockNode>

    <div v-else-if="!floating.length" class="dock-empty">
      <p>所有面板都已关闭。</p>
      <p class="hint">用工具栏「布局」菜单重新打开，或点「重置布局」。</p>
    </div>

    <div
      v-for="panel in floatingSorted"
      :key="panel.tab"
      class="dock-float"
      :style="{
        left: `${panel.x}px`,
        top: `${panel.y}px`,
        width: `${panel.width}px`,
        height: `${panel.height}px`,
        zIndex: 9000 + panel.z,
      }"
      @pointerdown="focus(panel.tab)"
    >
      <div
        class="dock-float-head"
        title="拖动移动；按住 Alt 可自由摆放，拖到面板上可重新停靠"
        @pointerdown="startFloatDrag(panel.tab, $event)"
      >
        <span class="dock-float-title">{{ titleOf(panel.tab) }}</span>
        <button
          class="dock-float-close"
          title="关闭面板"
          @pointerdown.stop
          @click.stop="close(panel.tab)"
        >
          <X :size="13" />
        </button>
      </div>
      <div class="dock-float-body">
        <slot :name="panel.tab" />
      </div>
      <div
        class="dock-float-resize"
        title="调整窗口大小"
        @pointerdown.stop="startFloatResize(panel.tab, $event)"
      />
    </div>

    <div v-if="edgeHover" class="dock-edge-drop" :class="`edge-${edgeHover}`">
      <span>
        停靠到窗口{{ edgeHover === "left" ? "左侧" : edgeHover === "right" ? "右侧" : "底部" }}
      </span>
    </div>

    <div
      v-if="dragPanel"
      class="dock-drag-ghost"
      :style="{ left: `${dragPos.x}px`, top: `${dragPos.y}px` }"
    >
      {{ titleOf(dragPanel) }}
    </div>
  </div>
</template>
