<script setup lang="ts">
import { computed, inject, ref } from "vue";
import { ASSETS_KEY } from "../keys";
import { formatBytes } from "../format";
import type { AssetInfo } from "../types";

/** 资产管理器（模态）：检索 + 分类过滤，选中一个路径返回给调用方。 */
const props = defineProps<{ current: string; title?: string }>();

const emit = defineEmits<{
  (e: "pick", path: string): void;
  (e: "cancel"): void;
}>();

const assets = inject(ASSETS_KEY, null);
const search = ref("");
const kind = ref("all");
const selected = ref(props.current);

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
  return ["all", ...Object.keys(KIND_LABELS).filter((key) => key !== "all" && present.has(key as AssetInfo["kind"]))];
});

const filtered = computed(() => {
  const query = search.value.trim().toLowerCase();
  return (assets?.value ?? [])
    .filter((asset) => kind.value === "all" || asset.kind === kind.value)
    .filter((asset) => !query || asset.path.toLowerCase().includes(query))
    .slice(0, 500);
});

function pick(path: string) {
  emit("pick", path);
}
</script>

<template>
  <div class="modal-mask" @click.self="emit('cancel')">
    <div class="modal">
      <header>
        <span class="title">{{ title ?? "选择资产" }}</span>
        <span class="spacer" />
        <button class="ghost icon" @click="emit('cancel')">✕</button>
      </header>

      <div class="modal-tools">
        <input v-model="search" type="text" placeholder="检索路径…" autofocus />
        <select v-model="kind">
          <option v-for="k in kinds" :key="k" :value="k">{{ KIND_LABELS[k] ?? k }}</option>
        </select>
      </div>

      <div class="modal-list">
        <div
          v-for="asset in filtered"
          :key="asset.path"
          class="list-item"
          :class="{ active: asset.path === selected }"
          @click="selected = asset.path"
          @dblclick="pick(asset.path)"
        >
          <span class="badge">{{ asset.kind }}</span>
          <span class="path">{{ asset.path }}</span>
          <span class="muted">{{ formatBytes(asset.sizeBytes) }}</span>
        </div>
        <div v-if="!filtered.length" class="hint" style="padding: 10px">没有匹配的资产</div>
      </div>

      <footer>
        <span class="hint" :title="selected">{{ selected || "（未选择）" }}</span>
        <span class="spacer" />
        <button class="ghost" @click="pick('')">清空</button>
        <button @click="emit('cancel')">取消</button>
        <button class="primary" :disabled="!selected" @click="pick(selected)">选择</button>
      </footer>
    </div>
  </div>
</template>
