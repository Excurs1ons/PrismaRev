<script setup lang="ts">
import { computed } from "vue";
import type { EntityJson } from "../types";

/** 实体层级树：由 `parent` 索引推导，渲染为带缩进的扁平列表。 */
const props = defineProps<{
  entities: EntityJson[];
  selected: number | null;
}>();

const emit = defineEmits<{ (e: "select", index: number): void }>();

interface Row {
  index: number;
  depth: number;
  label: string;
  children: number;
}

const rows = computed<Row[]>(() => {
  const count = props.entities.length;
  const children: number[][] = Array.from({ length: count }, () => []);
  const roots: number[] = [];

  props.entities.forEach((entity, index) => {
    const parent = entity.parent;
    if (parent === null || parent === undefined || parent < 0 || parent >= count || parent === index) {
      roots.push(index);
    } else {
      children[parent].push(index);
    }
  });

  const out: Row[] = [];
  const seen = new Set<number>();
  const walk = (index: number, depth: number) => {
    if (seen.has(index)) return;
    seen.add(index);
    out.push({
      index,
      depth,
      label: props.entities[index].name || `#${index}`,
      children: children[index].length,
    });
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
  <div>
    <div v-for="row in rows" :key="row.index">
      <div
        class="list-item"
        :class="{ active: row.index === selected }"
        :style="{ paddingLeft: `${8 + row.depth * 14}px` }"
        @click="emit('select', row.index)"
      >
        <span class="muted">{{ row.index }}</span>
        <span>{{ row.label }}</span>
        <span v-if="row.children" class="muted">{{ row.children }}</span>
      </div>
    </div>
    <div v-if="!rows.length" class="hint" style="padding: 8px">（无实体）</div>
  </div>
</template>
