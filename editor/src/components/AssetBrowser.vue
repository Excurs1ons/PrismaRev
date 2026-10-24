<script setup lang="ts">
import { computed, inject, ref } from "vue";
import * as api from "../api";
import { ASSETS_KEY } from "../keys";
import { formatBytes } from "../format";
import type { AssetInfo, AssetRef } from "../types";

/** 侧栏「资产」面板：浏览 + 检索 + 查看引用位置 + 拖拽到字段。 */
const emit = defineEmits<{ (e: "jump", ref: AssetRef): void }>();

const assets = inject(ASSETS_KEY, null);

const search = ref("");
const kind = ref("all");
const selected = ref<AssetInfo | null>(null);
const usage = ref<AssetRef[]>([]);
const usageError = ref("");
const busy = ref(false);

const KIND_LABELS: Record<string, string> = {
  all: "全部",
  model: "模型",
  texture: "贴图",
  env: "环境",
  scene: "场景",
  package: "包",
  data: "数据",
  other: "其他",
};

const kinds = computed(() => {
  const present = new Set((assets?.value ?? []).map((asset) => asset.kind));
  return [
    "all",
    ...Object.keys(KIND_LABELS).filter((key) => key !== "all" && present.has(key as AssetInfo["kind"])),
  ];
});

const filtered = computed(() => {
  const query = search.value.trim().toLowerCase();
  return (assets?.value ?? [])
    .filter((asset) => kind.value === "all" || asset.kind === kind.value)
    .filter((asset) => !query || asset.path.toLowerCase().includes(query))
    .slice(0, 400);
});

const total = computed(() => (assets?.value ?? []).length);

async function select(asset: AssetInfo) {
  selected.value = asset;
  usage.value = [];
  usageError.value = "";
  busy.value = true;
  try {
    usage.value = await api.assetUsage(asset.path);
  } catch (e) {
    usageError.value = `${e}`;
  } finally {
    busy.value = false;
  }
}

async function copyPath() {
  if (!selected.value) return;
  try {
    await navigator.clipboard.writeText(selected.value.path);
  } catch {
    // 剪贴板不可用时忽略（浏览器权限）。
  }
}

/** 拖拽到字段：自定义 MIME，避免和层级树的拖拽互相干扰。 */
function onDragStart(asset: AssetInfo, event: DragEvent) {
  event.dataTransfer?.setData("application/x-prism-asset", asset.path);
  event.dataTransfer?.setData("text/plain", asset.path);
  if (event.dataTransfer) event.dataTransfer.effectAllowed = "copy";
}
</script>

<template>
  <div class="tree-tools">
    <input v-model="search" type="text" :placeholder="`检索 ${total} 个资产…`" />
    <select v-model="kind" style="margin-top: 4px">
      <option v-for="k in kinds" :key="k" :value="k">{{ KIND_LABELS[k] ?? k }}</option>
    </select>
  </div>

  <div
    v-for="asset in filtered"
    :key="asset.path"
    class="list-item"
    :class="{ active: asset.path === selected?.path }"
    draggable="true"
    title="可拖到路径字段"
    @dragstart="onDragStart(asset, $event)"
    @click="select(asset)"
  >
    <span class="badge">{{ asset.kind }}</span>
    <span class="path">{{ asset.path }}</span>
    <span class="muted">{{ formatBytes(asset.sizeBytes) }}</span>
  </div>
  <div v-if="!filtered.length" class="hint" style="padding: 8px">没有匹配的资产</div>

  <div v-if="selected" class="asset-detail">
    <div class="hint path">{{ selected.path }}</div>
    <div class="hint">
      引用：<template v-if="busy">…</template>
      <template v-else-if="usageError">{{ usageError }}</template>
      <template v-else-if="usage.length">{{ usage.length }} 处</template>
      <template v-else>未被任何场景引用</template>
    </div>
    <button
      v-for="(ref, i) in usage"
      :key="i"
      class="usage-link"
      title="打开该文档并定位到这个字段"
      @click="emit('jump', ref)"
    >
      {{ ref.scene }} · #{{ ref.entityIndex }} {{ ref.entity }} ·
      {{ ref.component.split("::").pop() }}.{{ ref.field }}
    </button>
    <button class="ghost icon" style="margin-top: 4px" @click="copyPath">复制路径</button>
  </div>
</template>
