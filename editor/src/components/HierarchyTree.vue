<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref } from "vue";
import {
  Box,
  ChevronDown,
  ChevronRight,
  Eye,
  EyeOff,
  Plus,
  Search,
  X,
} from "lucide-vue-next";
import { clampMenuPosition } from "../contextMenu";
import { t } from "../i18n";
import type { EntityJson } from "../types";

/**
 * 实体层级树（Unity 7 Hierarchy 标准实现）：
 * - 单击选中，`Ctrl/Cmd+单击` 切换多选，`Shift+单击` 按可见顺序范围多选；
 * - 预制体高亮：实体拥有 `PrefabInstance` 组件或属于预制体实例时，文本呈现 Unity 标志性亮蓝色（#4da6ff），根实体带有蓝色立方体图标；
 * - 双击名字就地重命名（回车/失焦提交，Esc 取消）；
 * - 拖拽重排与嵌套：落在行的上/下缘 = 作为兄弟插入其前后，落在行中间 = 挂为其子级，落在底部空白 = 变为根实体；
 * - 级联创建菜单（Cascading Menu）：`+` 按钮或右键弹出对齐 Unity 的级联创建菜单（Create Empty / 3D Object / Light / Camera / UI）。
 */
const props = defineProps<{
  entities: EntityJson[];
  selected: number[];
  canAdd?: boolean;
  sceneName?: string;
  dirty?: boolean;
}>();

const emit = defineEmits<{
  (e: "select", indices: number[]): void;
  (e: "add", presetId?: string, parentIndex?: number | null): void;
  (e: "rename", index: number, name: string): void;
  (e: "reparent", child: number, parent: number | null): void;
  (e: "move", child: number, at: number, parent: number | null): void;
  (e: "duplicate", index: number): void;
  (e: "remove", indices: number[]): void;
  (e: "toggle-active", index: number, active: boolean): void;
  (e: "insert-prefab", path: string, parent: number | null): void;
}>();

interface Row {
  index: number;
  depth: number;
  label: string;
  hasChildren: boolean;
  collapsed: boolean;
  active: boolean;
  isPrefabRoot: boolean;
  isPrefab: boolean;
}

const filter = ref("");
const collapsed = ref<Set<number>>(new Set());
const anchor = ref<number | null>(null);
const renaming = ref<number | null>(null);
const renameText = ref("");
const dragging = ref<number | null>(null);
const dropTarget = ref<{ index: number; zone: "before" | "after" | "inside" } | null>(null);
const rootDropActive = ref(false);
const ENTITY_MIME = "application/x-prism-entity";
const ASSET_MIME = "application/x-prism-asset";

function draggedPrefab(event: DragEvent): string | null {
  const path = event.dataTransfer?.getData(ASSET_MIME) ?? "";
  return path.toLowerCase().endsWith(".prefab.json") ? path : null;
}

// ---------------------------------------------------------------------------
// 预制体判定
// ---------------------------------------------------------------------------

const PREFAB_COMPONENT = "prism_engine::scene::PrefabInstance";

function isPrefabRoot(index: number): boolean {
  return !!props.entities[index]?.components?.[PREFAB_COMPONENT];
}

function isPartOfPrefab(index: number): boolean {
  let cur: number | null = index;
  while (cur !== null && cur !== undefined) {
    if (isPrefabRoot(cur)) return true;
    cur = parentIndexOf(cur);
  }
  return false;
}

// ---------------------------------------------------------------------------
// 级联上下文与创建菜单
// ---------------------------------------------------------------------------

interface ContextMenuState {
  x: number;
  y: number;
  targetIndex: number | null; // null 表示在空白处或 "+" 按钮触发
  flipLeft?: boolean;
}

const contextMenu = ref<ContextMenuState | null>(null);
const activeSubmenu = ref<string | null>(null);

function openContextMenu(index: number, event: MouseEvent) {
  if (!props.selected.includes(index)) emit("select", [index]);
  const pos = clampMenuPosition(event.clientX, event.clientY, 200, 320);
  const flipLeft = event.clientX + 200 + 170 > window.innerWidth;
  contextMenu.value = { targetIndex: index, ...pos, flipLeft };
  activeSubmenu.value = null;
}

function openEmptyContextMenu(event: MouseEvent) {
  emit("select", []);
  const pos = clampMenuPosition(event.clientX, event.clientY, 200, 260);
  const flipLeft = event.clientX + 200 + 170 > window.innerWidth;
  contextMenu.value = { targetIndex: null, ...pos, flipLeft };
  activeSubmenu.value = null;
}

function openAddMenu(event: MouseEvent) {
  const rect = (event.currentTarget as HTMLElement).getBoundingClientRect();
  const pos = clampMenuPosition(rect.left, rect.bottom + 4, 200, 260);
  const flipLeft = rect.left + 200 + 170 > window.innerWidth;
  const targetIndex = props.selected.length ? props.selected[props.selected.length - 1] : null;
  contextMenu.value = { targetIndex, ...pos, flipLeft };
  activeSubmenu.value = null;
}

function closeContextMenu() {
  contextMenu.value = null;
  activeSubmenu.value = null;
}

function triggerCreate(presetId: string) {
  const target = contextMenu.value?.targetIndex ?? null;
  closeContextMenu();
  emit("add", presetId, target);
}

function runContextAction(action: "duplicate" | "remove" | "rename") {
  const target = contextMenu.value;
  closeContextMenu();
  if (!target || target.targetIndex === null) return;
  if (action === "duplicate") {
    emit("duplicate", target.targetIndex);
    return;
  }
  if (action === "rename") {
    startRename(target.targetIndex);
    return;
  }
  if (action === "remove") {
    const indices = props.selected.includes(target.targetIndex)
      ? [...props.selected]
      : [target.targetIndex];
    emit("remove", indices);
  }
}

onMounted(() => {
  window.addEventListener("pointerdown", closeContextMenu);
  window.addEventListener("scroll", closeContextMenu, true);
  window.addEventListener("blur", closeContextMenu);
  window.addEventListener("resize", closeContextMenu);
});

onUnmounted(() => {
  window.removeEventListener("pointerdown", closeContextMenu);
  window.removeEventListener("scroll", closeContextMenu, true);
  window.removeEventListener("blur", closeContextMenu);
  window.removeEventListener("resize", closeContextMenu);
});

// ---------------------------------------------------------------------------
// 树折叠与行展开
// ---------------------------------------------------------------------------

function toggle(index: number) {
  const next = new Set(collapsed.value);
  if (next.has(index)) {
    next.delete(index);
  } else {
    next.add(index);
  }
  collapsed.value = next;
}

const idToIndex = computed(() => {
  const map = new Map<number, number>();
  props.entities.forEach((entity, index) => {
    if (typeof entity.id === "number") {
      map.set(entity.id, index);
    }
  });
  return map;
});

const rows = computed<Row[]>(() => {
  const count = props.entities.length;
  const children: number[][] = Array.from({ length: count }, () => []);
  const roots: number[] = [];
  const map = idToIndex.value;

  props.entities.forEach((entity, index) => {
    const parentId = entity.parent;
    if (parentId === null || parentId === undefined) {
      roots.push(index);
      return;
    }
    const parentIndex = map.get(parentId);
    if (parentIndex === undefined || parentIndex === index) {
      roots.push(index);
    } else {
      children[parentIndex].push(index);
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
      const activeValue = props.entities[index].components?.["prism_engine::scene::Active"];
      const prefabRoot = isPrefabRoot(index);
      const partOfPrefab = prefabRoot || isPartOfPrefab(index);

      out.push({
        index,
        depth,
        label,
        hasChildren: children[index].length > 0,
        collapsed: folded,
        active: activeValue !== false,
        isPrefabRoot: prefabRoot,
        isPrefab: partOfPrefab,
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

function parentIndexOf(index: number): number | null {
  const parentId = props.entities[index]?.parent;
  if (parentId === null || parentId === undefined) return null;
  return idToIndex.value.get(parentId) ?? null;
}

// ---------------------------------------------------------------------------
// 选择逻辑
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// 就地重命名
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// 拖拽与树结构修改
// ---------------------------------------------------------------------------

function onDragStart(index: number, event: DragEvent) {
  dragging.value = index;
  const name = props.entities[index]?.name ?? "entity";
  event.dataTransfer?.setData(ENTITY_MIME, JSON.stringify({ index, name }));
  event.dataTransfer?.setData("text/plain", String(index));
  if (event.dataTransfer) event.dataTransfer.effectAllowed = "copyMove";
}

function onDragEnd() {
  dragging.value = null;
  dropTarget.value = null;
  rootDropActive.value = false;
}

function onDragOver(index: number, event: DragEvent) {
  if (dragging.value === null) {
    if (event.dataTransfer?.types.includes(ASSET_MIME)) {
      event.preventDefault();
      event.dataTransfer.dropEffect = "copy";
      dropTarget.value = { index, zone: "inside" };
    }
    return;
  }
  event.preventDefault();
  if (event.dataTransfer) event.dataTransfer.dropEffect = "move";

  const element = event.currentTarget as HTMLElement;
  const rect = element.getBoundingClientRect();
  const ratio = (event.clientY - rect.top) / Math.max(rect.height, 1);
  const zone = ratio < 0.25 ? "before" : ratio > 0.75 ? "after" : "inside";
  dropTarget.value = { index, zone };
  rootDropActive.value = false;
}

function onDrop(index: number, event: DragEvent) {
  event.preventDefault();
  const prefab = draggedPrefab(event);
  if (prefab) {
    dropTarget.value = null;
    rootDropActive.value = false;
    emit("insert-prefab", prefab, index);
    return;
  }
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
  emit("move", child, at, parentIndexOf(index));
}

function onRootDragOver(event: DragEvent) {
  if (dragging.value === null && !event.dataTransfer?.types.includes(ASSET_MIME)) return;
  event.preventDefault();
  rootDropActive.value = true;
}

function onRootDrop(event: DragEvent) {
  event.preventDefault();
  const prefab = draggedPrefab(event);
  if (prefab) {
    rootDropActive.value = false;
    dropTarget.value = null;
    emit("insert-prefab", prefab, null);
    return;
  }
  const child = dragging.value;
  rootDropActive.value = false;
  dragging.value = null;
  dropTarget.value = null;
  if (child !== null) emit("reparent", child, null);
}
</script>

<template>
  <!-- 顶部 Unity 风格工具栏 -->
  <div class="tree-tools">
    <div class="hierarchy-filter-row">
      <button
        class="ghost icon hierarchy-add"
        :disabled="!canAdd"
        :title="t('hierarchy.createTooltip')"
        @click.stop="openAddMenu"
      >
        <Plus :size="14" />
      </button>
      <div class="hierarchy-search-wrapper">
        <Search :size="12" class="search-icon" />
        <input
          v-model="filter"
          type="text"
          :placeholder="t('hierarchy.filterPlaceholder')"
        />
        <button
          v-if="filter"
          class="ghost icon clear-search"
          @click="filter = ''"
        >
          <X :size="12" />
        </button>
      </div>
    </div>
  </div>

  <!-- 场景根节点标头 -->
  <div v-if="sceneName" class="scene-root-row" @contextmenu.prevent.stop="openEmptyContextMenu">
    <span>{{ sceneName }}</span>
    <span v-if="dirty" class="scene-dirty-star" :title="t('common.unsaved')">*</span>
  </div>

  <!-- 树列表容器 -->
  <div
    class="hierarchy-tree-scroll"
    @contextmenu.prevent.stop="openEmptyContextMenu"
    @dragover="onRootDragOver"
    @drop="onRootDrop"
  >
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
      @contextmenu.prevent.stop="openContextMenu(row.index, $event)"
    >
      <div
        class="list-item"
        :class="{
          active: selected.includes(row.index),
          inactive: !row.active,
          'is-prefab': row.isPrefab,
        }"
        :style="{ paddingLeft: `${6 + row.depth * 14}px` }"
        @click="onRowClick(row.index, $event)"
      >
        <button
          v-if="row.hasChildren"
          class="ghost icon caret"
          :title="row.collapsed ? t('common.expand') : t('common.collapse')"
          @click.stop="toggle(row.index)"
        >
          <ChevronRight v-if="row.collapsed" :size="12" />
          <ChevronDown v-else :size="12" />
        </button>
        <span v-else class="caret-spacer" />

        <!-- 预制体蓝色立方体图标 -->
        <Box v-if="row.isPrefabRoot" :size="12" class="prefab-cube-icon" />

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
        <span
          v-else
          class="label"
          :class="{ 'prefab-label': row.isPrefab }"
          @dblclick.stop="startRename(row.index)"
        >
          {{ row.label }}
        </span>

        <button
          class="ghost icon visibility-toggle"
          :title="row.active ? t('hierarchy.disableEntity') : t('hierarchy.enableEntity')"
          @click.stop="emit('toggle-active', row.index, !row.active)"
        >
          <Eye v-if="row.active" :size="13" />
          <EyeOff v-else :size="13" />
        </button>
      </div>
    </div>

    <div v-if="!rows.length" class="hint" style="padding: 12px; text-align: center;">
      {{ filter ? t("hierarchy.noMatching") : t("hierarchy.empty") }}
    </div>

    <div
      v-if="dragging !== null"
      class="root-drop"
      :class="{ active: rootDropActive }"
      @dragover="onRootDragOver"
      @drop="onRootDrop"
    >
      {{ t("hierarchy.dropToRoot") }}
    </div>
  </div>

  <!-- 级联上下文菜单 (Cascading Create & Action Menu) -->
  <Teleport to="body">
    <div
      v-if="contextMenu"
      class="entity-context-menu"
      :style="{ left: `${contextMenu.x}px`, top: `${contextMenu.y}px` }"
      @pointerdown.stop
      @click.stop
      @contextmenu.prevent
    >
      <!-- 实体操作项（在实体上右键时可见） -->
      <template v-if="contextMenu.targetIndex !== null">
        <button @click="runContextAction('duplicate')">
          <span>{{ t("common.duplicate") }}</span>
          <span class="context-shortcut">Ctrl+D</span>
        </button>
        <button @click="runContextAction('rename')">
          <span>{{ t("hierarchy.rename") }}</span>
          <span class="context-shortcut">F2</span>
        </button>
        <button class="danger" @click="runContextAction('remove')">
          <span>{{ t("common.delete") }}</span>
          <span class="context-shortcut">Del</span>
        </button>
        <div class="context-menu-separator" />
      </template>

      <!-- 创建菜单选项 -->
      <button @click="triggerCreate('empty')">
        <span>{{ contextMenu.targetIndex !== null ? t("hierarchy.createEmptyChild") : t("hierarchy.createEmpty") }}</span>
      </button>

      <!-- 3D Object 级联子菜单 -->
      <div
        class="context-submenu-wrapper"
        @mouseenter="activeSubmenu = '3d'"
      >
        <button class="has-submenu">
          <span>{{ t("hierarchy.object3D") }}</span>
          <ChevronRight :size="12" class="submenu-arrow" />
        </button>
        <div
          v-if="activeSubmenu === '3d'"
          class="context-submenu"
          :class="{ 'flip-left': contextMenu.flipLeft }"
        >
          <button @click="triggerCreate('cube')">
            <span>{{ t("hierarchy.cube") }}</span>
          </button>
          <button @click="triggerCreate('sphere')">
            <span>{{ t("hierarchy.sphere") }}</span>
          </button>
          <button @click="triggerCreate('cylinder')">
            <span>{{ t("hierarchy.cylinder") }}</span>
          </button>
          <button @click="triggerCreate('plane')">
            <span>{{ t("hierarchy.plane") }}</span>
          </button>
          <button @click="triggerCreate('mesh')">
            <span>{{ t("hierarchy.customMesh") }}</span>
          </button>
        </div>
      </div>

      <!-- Light 级联子菜单 -->
      <div
        class="context-submenu-wrapper"
        @mouseenter="activeSubmenu = 'light'"
      >
        <button class="has-submenu">
          <span>{{ t("hierarchy.light") }}</span>
          <ChevronRight :size="12" class="submenu-arrow" />
        </button>
        <div
          v-if="activeSubmenu === 'light'"
          class="context-submenu"
          :class="{ 'flip-left': contextMenu.flipLeft }"
        >
          <button @click="triggerCreate('sun')">
            <span>{{ t("hierarchy.directionalLight") }}</span>
          </button>
          <button @click="triggerCreate('point_light')">
            <span>{{ t("hierarchy.pointLight") }}</span>
          </button>
          <button @click="triggerCreate('spot_light')">
            <span>{{ t("hierarchy.spotLight") }}</span>
          </button>
        </div>
      </div>

      <!-- Camera -->
      <button @click="triggerCreate('camera')">
        <span>{{ t("hierarchy.camera") }}</span>
      </button>

      <!-- Effects 级联子菜单 -->
      <div
        class="context-submenu-wrapper"
        @mouseenter="activeSubmenu = 'effects'"
      >
        <button class="has-submenu">
          <span>{{ t("hierarchy.effects") }}</span>
          <ChevronRight :size="12" class="submenu-arrow" />
        </button>
        <div
          v-if="activeSubmenu === 'effects'"
          class="context-submenu"
          :class="{ 'flip-left': contextMenu.flipLeft }"
        >
          <button @click="triggerCreate('skybox')">
            <span>{{ t("hierarchy.skybox") }}</span>
          </button>
        </div>
      </div>

      <!-- UI 级联子菜单 -->
      <div
        class="context-submenu-wrapper"
        @mouseenter="activeSubmenu = 'ui'"
      >
        <button class="has-submenu">
          <span>{{ t("hierarchy.ui") }}</span>
          <ChevronRight :size="12" class="submenu-arrow" />
        </button>
        <div
          v-if="activeSubmenu === 'ui'"
          class="context-submenu"
          :class="{ 'flip-left': contextMenu.flipLeft }"
        >
          <button @click="triggerCreate('ui')">
            <span>{{ t("hierarchy.uiNode") }}</span>
          </button>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<style scoped>
.hierarchy-filter-row {
  display: flex;
  align-items: center;
  gap: 4px;
  height: 28px;
  padding: 0 6px;
  background: var(--panel);
  border-bottom: 1px solid var(--border);
}

.hierarchy-search-wrapper {
  position: relative;
  display: flex;
  align-items: center;
  flex: 1 1 auto;
}

.hierarchy-search-wrapper .search-icon {
  position: absolute;
  left: 6px;
  color: var(--muted);
  pointer-events: none;
}

.hierarchy-search-wrapper input {
  width: 100%;
  height: 20px;
  padding: 0 20px 0 22px;
  font-size: 11px;
  background: var(--bg-sunken);
  border: 1px solid var(--border);
  border-radius: 3px;
  color: var(--text);
}

.clear-search {
  position: absolute;
  right: 2px;
  padding: 2px;
  height: 16px;
  color: var(--muted);
}

.hierarchy-tree-scroll {
  flex: 1 1 auto;
  min-height: 0;
  overflow-y: auto;
  overflow-x: hidden;
}

/* 预制体高亮 */
.prefab-label {
  color: #4da6ff !important;
  font-weight: 500;
}

.prefab-cube-icon {
  color: #4da6ff;
  flex-shrink: 0;
  margin-right: 4px;
}

.tree-row .list-item.active .prefab-label {
  color: #fff !important;
}

.tree-row .list-item.active .prefab-cube-icon {
  color: #fff !important;
}

/* 级联子菜单 */
.context-submenu-wrapper {
  position: relative;
}

.has-submenu {
  display: flex;
  align-items: center;
  justify-content: space-between;
  width: 100%;
}

.submenu-arrow {
  opacity: 0.6;
  margin-left: 8px;
}

.context-submenu {
  position: absolute;
  left: 100%;
  top: -4px;
  min-width: 160px;
  background: var(--panel-raised);
  border: 1px solid var(--border-strong);
  border-radius: 6px;
  box-shadow: 0 14px 34px rgba(0, 0, 0, 0.55);
  padding: 4px;
  z-index: 12100;
}

.context-submenu.flip-left {
  left: auto;
  right: 100%;
}
</style>
