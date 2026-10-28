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
  resizeFloatRect,
  setRatio,
  snapFloatPosition,
  type DockEdge,
  type DockNode as DockNodeType,
  type DockZone,
  type FloatPanel,
  type FloatRect,
  type ResizeDirection,
} from "../dock";
import { DOCK_KEY, type DockHover } from "../dockContext";

const props = defineProps<{
  panels: Array<{ id: string; title: string }>;
}>();

const layout = defineModel<DockNodeType | null>("layout", { required: true });
const floating = defineModel<FloatPanel[]>("floating", { default: () => [] });

const rootEl = ref<HTMLElement | null>(null);
const dragPanel = ref<string | null>(null);
const draggingFloatTab = ref<string | null>(null);
const hover = ref<DockHover | null>(null);
const edgeHover = ref<DockEdge | null>(null);
const activeSnapLines = ref<Array<{ type: "vertical" | "horizontal"; coord: number }>>([]);
const dragPos = reactive({ x: 0, y: 0 });
/** 只允许一个拖拽会话，避免旧 pointer 监听器在新拖拽的 pointerup 上重复触发。 */
let dragSeq = 0;
/** 当前拖拽会话的清理函数；组件卸载时用它移除 window 监听器。 */
let endActiveDrag: (() => void) | null = null;

const titles = computed(() => new Map(props.panels.map((panel) => [panel.id, panel.title])));
const floatingSorted = computed(() => [...floating.value].sort((a, b) => a.z - b.z));
const maxZ = computed(() => floating.value.reduce((max, panel) => Math.max(max, panel.z), 0));

function titleOf(id: string): string {
  return titles.value.get(id) ?? id;
}

function nextZ(): number {
  return maxZ.value + 1;
}

function isFocused(panel: FloatPanel): boolean {
  return floating.value.length > 0 && panel.z === maxZ.value;
}

/** 点击/拖动浮动面板时置顶。 */
function focus(tab: string) {
  const panel = floating.value.find((item) => item.tab === tab);
  if (!panel) return;
  const currentMax = maxZ.value;
  if (panel.z === currentMax && floating.value.length > 0) return;
  const z = currentMax + 1;
  let next = floating.value.map((item) => (item.tab === tab ? { ...item, z } : item));
  if (z > 200) {
    const sorted = [...next].sort((a, b) => a.z - b.z);
    next = sorted.map((item, idx) => ({ ...item, z: idx + 1 }));
  }
  floating.value = next;
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

function drop(leafId: string, zone: DockZone, tab: string, targetIndex?: number) {
  floating.value = floating.value.filter((panel) => panel.tab !== tab);
  if (!layout.value) {
    layout.value = leaf([tab]);
    dragPanel.value = null;
    hover.value = null;
    return;
  }
  layout.value = insertPanel(layout.value, leafId, tab, zone, 0.5, targetIndex);
  dragPanel.value = null;
  hover.value = null;
}

interface ZoneResult {
  zone: DockZone;
  targetIndex?: number;
}

function zoneAt(leafEl: HTMLElement, x: number, y: number): ZoneResult {
  const rect = leafEl.getBoundingClientRect();
  const relX = x - rect.left;
  const relY = y - rect.top;
  const w = Math.max(1, rect.width);
  const h = Math.max(1, rect.height);

  // 1. 如果光标落在顶部的标签栏区域（高度 ~32px）或靠近标签栏，直接判定为标签页堆叠
  const tabsEl = leafEl.querySelector<HTMLElement>(".dock-tabs");
  if (tabsEl) {
    const tabsRect = tabsEl.getBoundingClientRect();
    if (y >= tabsRect.top - 6 && y <= tabsRect.bottom + 8) {
      // 计算插入的具体标签索引，实现顺滑插队
      const tabEls = Array.from(tabsEl.querySelectorAll<HTMLElement>(".dock-tab"));
      let targetIndex = tabEls.length;
      for (let i = 0; i < tabEls.length; i++) {
        const tr = tabEls[i].getBoundingClientRect();
        if (x < tr.left + tr.width / 2) {
          targetIndex = i;
          break;
        }
      }
      return { zone: "center", targetIndex };
    }
  }

  // 2. 边框吸附区域计算（根据窗口大小动态适应，最大 52px，最小 24px，约占尺寸的 20%）
  const edgeX = Math.max(24, Math.min(w * 0.2, 52));
  const edgeY = Math.max(24, Math.min(h * 0.2, 52));

  const distLeft = relX;
  const distRight = w - relX;
  const distTop = relY;
  const distBottom = h - relY;

  const isLeft = distLeft < edgeX;
  const isRight = distRight < edgeX;
  const isTop = distTop < edgeY;
  const isBottom = distBottom < edgeY;

  if (isLeft || isRight || isTop || isBottom) {
    const minDist = Math.min(
      isLeft ? distLeft : Infinity,
      isRight ? distRight : Infinity,
      isTop ? distTop : Infinity,
      isBottom ? distBottom : Infinity,
    );
    if (minDist === distLeft) return { zone: "left" };
    if (minDist === distRight) return { zone: "right" };
    if (minDist === distTop) return { zone: "top" };
    if (minDist === distBottom) return { zone: "bottom" };
  }

  // 其余中央主体区域（约 65% ~ 80%）全部为合并堆叠
  return { zone: "center" };
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
  const result = zoneAt(leafEl, x, y);
  hover.value = {
    leafId: leafEl.dataset.leafId,
    zone: result.zone,
    targetIndex: result.targetIndex,
  };
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
  endActiveDrag?.();
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
    removeListeners();
    if (seq !== dragSeq) return;
    document.body.style.userSelect = "";
    document.body.style.cursor = "";

    if (started) {
      const edge = edgeHover.value;
      const target = hover.value;
      if (edge && !forceFloat) dockEdge(tab, edge);
      else if (target && !forceFloat) drop(target.leafId, target.zone, tab, target.targetIndex);
      else detach(tab, dragPos.x, dragPos.y);
    }
    dragPanel.value = null;
    hover.value = null;
    edgeHover.value = null;
  };

  function removeListeners() {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", finish);
    window.removeEventListener("pointercancel", cancel);
    if (endActiveDrag === cancel) endActiveDrag = null;
  }

  function cancel() {
    removeListeners();
    if (seq !== dragSeq) return;
    document.body.style.userSelect = "";
    document.body.style.cursor = "";
    dragPanel.value = null;
    hover.value = null;
    edgeHover.value = null;
  }

  endActiveDrag = cancel;
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", finish);
  window.addEventListener("pointercancel", cancel);
}

/** 拖动浮动窗口标题栏；放到 dock 区域会重新停靠。 */
function startFloatDrag(tab: string, event: PointerEvent) {
  if (event.button !== 0) return;
  endActiveDrag?.();
  const seq = ++dragSeq;
  focus(tab);
  draggingFloatTab.value = tab;
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
    let targetX = originX + dx;
    let targetY = originY + dy;

    if (!forceFloat && !hover.value && !edgeHover.value) {
      const otherPanels = floating.value.filter((item) => item.tab !== tab);
      const snap = snapFloatPosition(
        { x: targetX, y: targetY, width: panel.width, height: panel.height },
        otherPanels,
        window.innerWidth,
        window.innerHeight,
      );
      targetX = snap.x;
      targetY = snap.y;
      activeSnapLines.value = snap.snapLines;
    } else {
      activeSnapLines.value = [];
    }

    floating.value = floating.value.map((item) =>
      item.tab === tab ? { ...item, x: targetX, y: targetY } : item,
    );
  };

  const finish = () => {
    removeListeners();
    if (seq !== dragSeq) return;
    document.body.style.userSelect = "";
    document.body.style.cursor = "";
    draggingFloatTab.value = null;
    activeSnapLines.value = [];

    const edge = edgeHover.value;
    const target = hover.value;
    if (edge && !forceFloat) {
      dockEdge(tab, edge);
    } else if (target && !forceFloat) {
      drop(target.leafId, target.zone, tab, target.targetIndex);
    } else {
      floating.value = floating.value.map((item) =>
        item.tab === tab ? clampFloatPanel(item, window.innerWidth, window.innerHeight) : item,
      );
    }
    hover.value = null;
    edgeHover.value = null;
  };

  function removeListeners() {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", finish);
    window.removeEventListener("pointercancel", cancel);
    if (endActiveDrag === cancel) endActiveDrag = null;
  }

  function cancel() {
    removeListeners();
    if (seq !== dragSeq) return;
    document.body.style.userSelect = "";
    document.body.style.cursor = "";
    draggingFloatTab.value = null;
    activeSnapLines.value = [];
    hover.value = null;
    edgeHover.value = null;
  }

  endActiveDrag = cancel;
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", finish);
  window.addEventListener("pointercancel", cancel);
}

/** 8 方向调整浮动窗口尺寸。 */
function startFloatResize(tab: string, dir: ResizeDirection, event: PointerEvent) {
  if (event.button !== 0) return;
  endActiveDrag?.();
  const seq = ++dragSeq;
  event.preventDefault();
  focus(tab);
  const panel = floating.value.find((item) => item.tab === tab);
  if (!panel) return;
  const capture = event.currentTarget as HTMLElement | null;
  capture?.setPointerCapture?.(event.pointerId);
  const startX = event.clientX;
  const startY = event.clientY;
  const initial: FloatRect = {
    x: panel.x,
    y: panel.y,
    width: panel.width,
    height: panel.height,
  };

  const move = (ev: PointerEvent) => {
    if (seq !== dragSeq) return;
    const dx = ev.clientX - startX;
    const dy = ev.clientY - startY;
    const nextRect = resizeFloatRect(
      initial,
      dir,
      dx,
      dy,
      window.innerWidth,
      window.innerHeight,
    );
    floating.value = floating.value.map((item) =>
      item.tab === tab ? { ...item, ...nextRect } : item,
    );
  };

  const finish = () => {
    removeListeners();
    if (seq !== dragSeq) return;
    floating.value = floating.value.map((item) =>
      item.tab === tab ? clampFloatPanel(item, window.innerWidth, window.innerHeight) : item,
    );
  };

  function removeListeners() {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", finish);
    window.removeEventListener("pointercancel", cancel);
    if (endActiveDrag === cancel) endActiveDrag = null;
  }

  function cancel() {
    removeListeners();
  }

  endActiveDrag = cancel;
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", finish);
  window.addEventListener("pointercancel", cancel);
}

function clampAllFloating() {
  if (!floating.value.length) return;
  floating.value = floating.value.map((panel) =>
    clampFloatPanel(panel, window.innerWidth, window.innerHeight),
  );
}

onMounted(() => window.addEventListener("resize", clampAllFloating));
onUnmounted(() => {
  window.removeEventListener("resize", clampAllFloating);
  endActiveDrag?.();
  endActiveDrag = null;
});

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
      :class="{
        'is-focused': isFocused(panel),
        'is-dragging': draggingFloatTab === panel.tab,
      }"
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

      <!-- 8 方向尺寸调整边框与手柄 -->
      <div
        class="dock-float-resize edge-n"
        @pointerdown.stop="startFloatResize(panel.tab, 'n', $event)"
      />
      <div
        class="dock-float-resize edge-s"
        @pointerdown.stop="startFloatResize(panel.tab, 's', $event)"
      />
      <div
        class="dock-float-resize edge-w"
        @pointerdown.stop="startFloatResize(panel.tab, 'w', $event)"
      />
      <div
        class="dock-float-resize edge-e"
        @pointerdown.stop="startFloatResize(panel.tab, 'e', $event)"
      />
      <div
        class="dock-float-resize corner-nw"
        @pointerdown.stop="startFloatResize(panel.tab, 'nw', $event)"
      />
      <div
        class="dock-float-resize corner-ne"
        @pointerdown.stop="startFloatResize(panel.tab, 'ne', $event)"
      />
      <div
        class="dock-float-resize corner-sw"
        @pointerdown.stop="startFloatResize(panel.tab, 'sw', $event)"
      />
      <div
        class="dock-float-resize corner-se"
        title="调整窗口大小"
        @pointerdown.stop="startFloatResize(panel.tab, 'se', $event)"
      />
    </div>

    <!-- 磁吸对齐指示线 -->
    <template v-for="(line, idx) in activeSnapLines" :key="idx">
      <div
        v-if="line.type === 'vertical'"
        class="dock-snap-guide vertical"
        :style="{ left: `${line.coord}px` }"
      />
      <div
        v-else
        class="dock-snap-guide horizontal"
        :style="{ top: `${line.coord}px` }"
      />
    </template>

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
