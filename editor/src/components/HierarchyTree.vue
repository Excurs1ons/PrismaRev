<script setup lang="ts">
import { computed, ref } from "vue";
import type { EntityJson } from "../types";

/** 实体层级树：由 `parent` 索引推导，支持折叠与按名字过滤。 */
const props = defineProps<{
  entities: EntityJson[];
  selected: number | null;
}>();

const emit = defineEmits<{ (e: "select", index: number): void }>();

interface Row {
  index: number;
  depth: number;
  label: string;
  hasChildren: boolean;
  collapsed: boolean;
}

const filter = ref("");
const collapsed = ref<Set<number>>(new Set());

function toggle(index: number) {
  const next = new Set(collapsed.value);
  if (next.has(index)) {
    next.delete(index);
  } else {
    next.add(index);
  }
  collapsed.value = next;
}

const rows = computed<Row[]>(() => {
  const count = props.entities.length;
  const children: number[][] = Array.from({ length: count }, () => []);
  const roots: number[] = [];

  props.entities.forEach((entity, index) => {
    const parent = entity.parent;
    if (
      parent === null ||
      parent === undefined ||
      parent < 0 ||
      parent >= count ||
      parent === index
    ) {
      roots.push(index);
    } else {
      children[parent].push(index);
    }
  });

  const query = filter.value.trim().toLowerCase();
  const out: Row[] = [];
  const seen = new Set<number>();

  const walk = (index: number, depth: number) => {
    if (seen.has(index)) return;
    seen.add(index);

    const label = props.entities[index].name || `#${index}`;
    const folded = !query && collapsed.value.has(index);
    if (!query || label.toLowerCase().includes(query)) {
      out.push({
        index,
        depth,
        label,
        hasChildren: children[index].length > 0,
        collapsed: folded,
      });
    }
    // 过滤模式下即使父节点被折叠也要继续下探，保证子节点能被匹配到。
    if (folded) return;
    for (const child of children[index]) walk(child, depth + 1);
  };

  for (const root of roots) walk(root, 0);
  for (let index = 0; index < count; index++) {
    if (!seen.has(index)) walk(index, 0);
  }
  return out;
});
</script>

<template>
  <div class="tree-tools">
    <input v-model="filter" type="text" placeholder="过滤实体…" />
  </div>
  <div v-for="row in rows" :key="row.index">
    <div
      class="list-item"
      :class="{ active: row.index === selected }"
      :style="{ paddingLeft: `${6 + row.depth * 14}px` }"
      @click="emit('select', row.index)"
    >
      <button
        v-if="row.hasChildren"
        class="ghost icon caret"
        :title="row.collapsed ? '展开' : '折叠'"
        @click.stop="toggle(row.index)"
      >
        {{ row.collapsed ? "▸" : "▾" }}
      </button>
      <span v-else class="caret-spacer" />
      <span class="muted">{{ row.index }}</span>
      <span>{{ row.label }}</span>
    </div>
  </div>
  <div v-if="!rows.length" class="hint" style="padding: 8px">
    {{ filter ? "无匹配实体" : "（无实体）" }}
  </div>
</template>
