<script setup lang="ts">
import { computed, nextTick, ref } from "vue";
import type { EntityJson } from "../types";

/**
 * 实体层级树。
 *
 * - 单击选中，`Ctrl/Cmd+单击` 切换，`Shift+单击` 按可见顺序范围选择；
 * - 双击名字就地重命名（回车/失焦提交，Esc 取消）；
 * - 拖拽：落在行的上/下缘 = 作为兄弟插入其前后，落在行中间 = 挂为其子级，
 *   落在底部空白 = 变为根实体。
 */
const props = defineProps<{
  entities: EntityJson[];
  selected: number[];
}>();

const emit = defineEmits<{
  (e: "select", indices: number[]): void;
  (e: "rename", index: number, name: string): void;
  (e: "reparent", child: number, parent: number | null): void;
  (e: "move", child: number, at: number, parent: number | null): void;
}>();

interface Row {
  index: number;
  depth: number;
  label: string;
  hasChildren: boolean;
  collapsed: boolean;
}

const filter = ref("");
const collapsed = ref<Set<number>>(new Set());
const anchor = ref<number | null>(null);
const renaming = ref<number | null>(null);
const renameText = ref("");
const dragging = ref<number | null>(null);
const dropTarget = ref<{ index: number; zone: "before" | "after" | "inside" } | null>(null);
const rootDropActive = ref(false);

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
    if (folded) return;
    for (const child of children[index]) walk(child, depth + 1);
  };

  for (const root of roots) walk(root, 0);
  for (let index = 0; index < count; index++) {
    if (!seen.has(index)) walk(index, 0);
  }
  return out;
});

function parentOf(index: number): number | null {
  const parent = props.entities[index]?.parent;
  return parent === null || parent === undefined ? null : parent;
}

// ── 选择 ────────────────────────────────────────────────────────────────────

function onRowClick(index: number, event: MouseEvent) {
  if (renaming.value !== null) return;
  const visible = rows.value.map((row) => row.index);

  if (event.shiftKey && anchor.value !== null) {
    const from = visible.indexOf(anchor.value);
    const to = visible.indexOf(index);
    if (from >= 0 && to >= 0) {
      const [lo, hi] = from <= to ? [from, to] : [to, from];
      emit("select", visible.slice(lo, hi + 1));
      return;
    }
  }

  if (event.ctrlKey || event.metaKey) {
    const next = props.selected.includes(index)
      ? props.selected.filter((item) => item !== index)
      : [...props.selected, index];
    emit("select", next.length ? next : [index]);
    anchor.value = index;
    return;
  }

  emit("select", [index]);
  anchor.value = index;
}

// ── 就地重命名 ──────────────────────────────────────────────────────────────

function startRename(index: number) {
  renaming.value = index;
  renameText.value = props.entities[index]?.name ?? "";
  void nextTick(() => {
    const input = document.querySelector<HTMLInputElement>(".rename-input");
    input?.focus();
    input?.select();
  });
}

function commitRename() {
  if (renaming.value === null) return;
  const index = renaming.value;
  const name = renameText.value.trim();
  renaming.value = null;
  if (name && name !== (props.entities[index]?.name ?? "")) {
    emit("rename", index, name);
  }
}

function cancelRename() {
  renaming.value = null;
}

// ── 拖拽 ────────────────────────────────────────────────────────────────────

function onDragStart(index: number, event: DragEvent) {
  dragging.value = index;
  event.dataTransfer?.setData("text/plain", String(index));
  if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
}

function onDragEnd() {
  dragging.value = null;
  dropTarget.value = null;
  rootDropActive.value = false;
}

function onDragOver(index: number, event: DragEvent) {
  if (dragging.value === null) return;
  event.preventDefault();
  if (event.dataTransfer) event.dataTransfer.dropEffect = "move";

  const element = event.currentTarget as HTMLElement;
  const rect = element.getBoundingClientRect();
  const ratio = (event.clientY - rect.top) / Math.max(rect.height, 1);
  // 上/下 25% 作为兄弟插入，中间作为子级。
  const zone = ratio < 0.25 ? "before" : ratio > 0.75 ? "after" : "inside";
  dropTarget.value = { index, zone };
  rootDropActive.value = false;
}

function onDrop(index: number, event: DragEvent) {
  event.preventDefault();
  const child = dragging.value;
  const zone = dropTarget.value?.zone ?? "inside";
  dropTarget.value = null;
  dragging.value = null;
  rootDropActive.value = false;
  if (child === null) return;

  if (zone === "inside") {
    if (child !== index) emit("reparent", child, index);
    return;
  }
  const at = zone === "before" ? index : index + 1;
  emit("move", child, at, parentOf(index));
}

function onRootDragOver(event: DragEvent) {
  if (dragging.value === null) return;
  event.preventDefault();
  rootDropActive.value = true;
}

function onRootDrop(event: DragEvent) {
  event.preventDefault();
  const child = dragging.value;
  rootDropActive.value = false;
  dragging.value = null;
  dropTarget.value = null;
  if (child !== null) emit("reparent", child, null);
}
</script>

<template>
  <div class="tree-tools">
    <input v-model="filter" type="text" placeholder="过滤实体…" />
  </div>

  <div
    v-for="row in rows"
    :key="row.index"
    class="tree-row"
    :class="{
      dragging: dragging === row.index,
      'drop-before': dropTarget?.index === row.index && dropTarget.zone === 'before',
      'drop-after': dropTarget?.index === row.index && dropTarget.zone === 'after',
      'drop-inside': dropTarget?.index === row.index && dropTarget.zone === 'inside',
    }"
    draggable="true"
    @dragstart="onDragStart(row.index, $event)"
    @dragend="onDragEnd"
    @dragover="onDragOver(row.index, $event)"
    @drop="onDrop(row.index, $event)"
  >
    <div
      class="list-item"
      :class="{ active: selected.includes(row.index) }"
      :style="{ paddingLeft: `${6 + row.depth * 14}px` }"
      @click="onRowClick(row.index, $event)"
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
      <input
        v-if="renaming === row.index"
        v-model="renameText"
        class="rename-input"
        type="text"
        @click.stop
        @keydown.enter="commitRename"
        @keydown.esc="cancelRename"
        @blur="commitRename"
      />
      <span v-else class="label" @dblclick.stop="startRename(row.index)">{{ row.label }}</span>
    </div>
  </div>

  <div v-if="!rows.length" class="hint" style="padding: 8px">
    {{ filter ? "无匹配实体" : "（无实体）" }}
  </div>

  <div
    v-if="dragging !== null"
    class="root-drop"
    :class="{ active: rootDropActive }"
    @dragover="onRootDragOver"
    @drop="onRootDrop"
  >
    拖到这里成为根实体
  </div>
</template>
