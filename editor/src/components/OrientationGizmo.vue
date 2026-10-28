<script setup lang="ts">
/**
 * Unity 风格 SceneView 右上角 3D 方向坐标轴导航指示器 (Orientation Gizmo)。
 *
 * 实时反映当前相机视角的红绿蓝三轴方向 (X=红, Y=绿, Z=蓝)，支持点击对应方向锥体
 * 快速平滑对齐视口相机（Top, Bottom, Left, Right, Front, Back）。
 */
import { computed, ref } from "vue";
import type { CameraOrbit } from "../types";

const props = defineProps<{
  orbit: CameraOrbit;
}>();

const emit = defineEmits<{
  (event: "align", target: { yaw: number; pitch: number }): void;
  (event: "reset"): void;
}>();

const hoveredAxis = ref<string | null>(null);

interface AxisItem {
  id: string;
  name: string;
  color: string;
  labelColor: string;
  isNegative: boolean;
  targetYaw: number;
  targetPitch: number;
  x: number;
  y: number;
  depth: number;
}

const axes = computed<AxisItem[]>(() => {
  const yaw = props.orbit.yaw;
  const pitch = props.orbit.pitch;

  const sinY = Math.sin(yaw);
  const cosY = Math.cos(yaw);
  const sinP = Math.sin(pitch);
  const cosP = Math.cos(pitch);

  // 相机坐标基底在世界空间下的投影
  const rx = cosY;
  const rz = -sinY;
  const ux = sinY * sinP;
  const uy = cosP;
  const uz = cosY * sinP;
  const fx = sinY * cosP;
  const fy = -sinP;
  const fz = cosY * cosP;

  const rawAxes = [
    { id: "+x", name: "X", vec: [1, 0, 0], color: "#ef4444", labelColor: "#ffffff", isNegative: false, yaw: Math.PI / 2, pitch: 0 },
    { id: "-x", name: "",  vec: [-1, 0, 0], color: "#8b96ad", labelColor: "transparent", isNegative: true, yaw: -Math.PI / 2, pitch: 0 },
    { id: "+y", name: "Y", vec: [0, 1, 0], color: "#22c55e", labelColor: "#ffffff", isNegative: false, yaw, pitch: -1.5 },
    { id: "-y", name: "",  vec: [0, -1, 0], color: "#8b96ad", labelColor: "transparent", isNegative: true, yaw, pitch: 1.5 },
    { id: "+z", name: "Z", vec: [0, 0, 1], color: "#3b82f6", labelColor: "#ffffff", isNegative: false, yaw: 0, pitch: 0 },
    { id: "-z", name: "",  vec: [0, 0, -1], color: "#8b96ad", labelColor: "transparent", isNegative: true, yaw: Math.PI, pitch: 0 },
  ];

  const R = 28;
  const CX = 44;
  const CY = 44;

  const items: AxisItem[] = rawAxes.map((a) => {
    const [vx, vy, vz] = a.vec;
    const xScreen = vx * rx + vz * rz;
    const yScreen = -(vx * ux + vy * uy + vz * uz);
    const depth = vx * fx + vy * fy + vz * fz;

    return {
      id: a.id,
      name: a.name,
      color: a.color,
      labelColor: a.labelColor,
      isNegative: a.isNegative,
      targetYaw: a.yaw,
      targetPitch: a.pitch,
      x: CX + xScreen * R,
      y: CY + yScreen * R,
      depth,
    };
  });

  // 深度排序：远离视点的轴先绘制（画家算法）
  return items.sort((a, b) => a.depth - b.depth);
});

function clickAxis(axis: AxisItem) {
  emit("align", { yaw: axis.targetYaw, pitch: axis.targetPitch });
}
</script>

<template>
  <div class="orientation-gizmo" @pointerdown.stop>
    <svg class="gizmo-svg" viewBox="0 0 88 88">
      <!-- 轴线 -->
      <line
        v-for="axis in axes"
        :key="'line-' + axis.id"
        x1="44"
        y1="44"
        :x2="axis.x"
        :y2="axis.y"
        :stroke="axis.color"
        :stroke-width="axis.isNegative ? 1.5 : 2"
        :stroke-dasharray="axis.isNegative ? '2 2' : undefined"
        :opacity="axis.isNegative ? 0.6 : 0.85"
      />

      <!-- 中心核心点 -->
      <circle
        cx="44"
        cy="44"
        r="6.5"
        class="gizmo-center-core"
        :title="$t ? $t('viewport.resetView') : 'Reset View'"
        @click.stop="emit('reset')"
      />

      <!-- 轴端指示锥体 / 圆点 -->
      <g
        v-for="axis in axes"
        :key="'node-' + axis.id"
        class="axis-node-group"
        :class="{ hovered: hoveredAxis === axis.id }"
        @click.stop="clickAxis(axis)"
        @pointerenter="hoveredAxis = axis.id"
        @pointerleave="hoveredAxis = null"
      >
        <!-- 正向轴大端点 (带标签) -->
        <template v-if="!axis.isNegative">
          <circle
            :cx="axis.x"
            :cy="axis.y"
            r="8"
            :fill="axis.color"
            class="axis-node"
          />
          <text
            :x="axis.x"
            :y="axis.y + 3.5"
            text-anchor="middle"
            class="axis-label"
          >{{ axis.name }}</text>
        </template>

        <!-- 负向轴小圆点 -->
        <circle
          v-else
          :cx="axis.x"
          :cy="axis.y"
          r="4"
          :fill="axis.color"
          class="axis-neg-node"
        />
      </g>
    </svg>
    <div
      class="gizmo-mode-badge"
      :title="$t ? $t('viewport.resetView') : 'Reset View'"
      @click.stop="emit('reset')"
    >
      Persp
    </div>
  </div>
</template>

<style scoped>
.orientation-gizmo {
  position: absolute;
  top: 8px;
  right: 12px;
  width: 88px;
  display: flex;
  flex-direction: column;
  align-items: center;
  z-index: 20;
  user-select: none;
  filter: drop-shadow(0 2px 6px rgba(0, 0, 0, 0.4));
  transition: right 0.15s ease;
}

.orientation-gizmo.stats-open {
  right: 280px;
}

.gizmo-svg {
  width: 88px;
  height: 88px;
  overflow: visible;
}

.gizmo-center-core {
  fill: rgba(30, 32, 40, 0.85);
  stroke: rgba(255, 255, 255, 0.25);
  stroke-width: 1.5;
  cursor: pointer;
  transition: all 0.12s ease;
}

.gizmo-center-core:hover {
  fill: #3b82f6;
  stroke: #ffffff;
}

.axis-node-group {
  cursor: pointer;
}

.axis-node {
  stroke: rgba(0, 0, 0, 0.25);
  stroke-width: 1;
  transition: transform 0.12s ease, filter 0.12s ease;
}

.axis-node-group.hovered .axis-node {
  filter: brightness(1.25) drop-shadow(0 0 3px rgba(255, 255, 255, 0.6));
}

.axis-neg-node {
  stroke: rgba(0, 0, 0, 0.3);
  stroke-width: 1;
  transition: all 0.12s ease;
}

.axis-node-group.hovered .axis-neg-node {
  fill: #ffffff;
  filter: drop-shadow(0 0 3px rgba(255, 255, 255, 0.8));
}

.axis-label {
  fill: #ffffff;
  font-family: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
  font-size: 10px;
  font-weight: 700;
  pointer-events: none;
}

.gizmo-mode-badge {
  margin-top: -6px;
  padding: 1px 6px;
  border-radius: 3px;
  background: rgba(20, 22, 28, 0.75);
  border: 1px solid rgba(255, 255, 255, 0.12);
  color: #c4c4c4;
  font-size: 9.5px;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  cursor: pointer;
  transition: all 0.12s ease;
}

.gizmo-mode-badge:hover {
  color: #ffffff;
  background: rgba(30, 34, 45, 0.95);
  border-color: rgba(255, 255, 255, 0.3);
}
</style>
