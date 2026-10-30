<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { ChevronDown, ChevronRight, MoreVertical, X } from "lucide-vue-next";
import { clampMenuPosition } from "../contextMenu";
import EditorDropdown, { type DropdownOption } from "./EditorDropdown.vue";
import FieldEditor from "./FieldEditor.vue";
import { eulerToQuaternion, quaternionToEuler } from "../math";
import * as api from "../api";
import { t } from "../i18n";
import type { ComponentClipboard, EntityJson, FieldSchema, JsonValue, Schema } from "../types";

/**
 * 单实体检查器：Inspector 规范。
 * - 顶部 GameObjectHeader：激活复选框、名称输入、静态标记、Tag / Layer 下拉
 * - Transform 变换：支持欧拉角（度）与四元数无缝双向换算
 * - 组件容器：扁平卡片、折叠三角、右键 / 三点操作菜单
 * - 底部居中「添加组件」按钮与快速检索弹出层
 */
const props = defineProps<{
  entity: EntityJson;
  index: number;
  entities: EntityJson[];
  schema: Schema;
  selectionCount: number;
  clipboard: ComponentClipboard | null;
  /** 该实体若来自预制体，这里是预制体路径（相对项目根）。 */
  prefabLink: string | null;
  /** 「跳到使用处」时高亮的组件 / 字段。 */
  highlightComponent: string | null;
  highlightField: string | null;
}>();

const emit = defineEmits<{
  (e: "update", entity: EntityJson, editKey: string): void;
  (e: "revert-prefab", index: number): void;
  (e: "apply-prefab", index: number): void;
  (e: "unlink-prefab", index: number): void;
  (e: "copy-component", name: string): void;
  (e: "paste-component"): void;
  (e: "clear-clipboard"): void;
  (e: "reflect", component: string, canonical: JsonValue, index: number, pending: JsonValue): void;
  (e: "rejected", message: string): void;
}>();

const addName = ref("");
const filter = ref("");
const showAddPopup = ref(false);
const addFilter = ref("");
const showEuler = ref(true);
const collapsedCards = ref<Set<string>>(new Set());
const componentMenu = ref<{ name: string; x: number; y: number } | null>(null);

function toggleCard(name: string) {
  const set = new Set(collapsedCards.value);
  if (set.has(name)) {
    set.delete(name);
  } else {
    set.add(name);
  }
  collapsedCards.value = set;
}

function openComponentMenu(name: string, event: MouseEvent) {
  const pos = clampMenuPosition(event.clientX, event.clientY, 190, 82);
  componentMenu.value = { name, ...pos };
}

function closeComponentMenu() {
  componentMenu.value = null;
}

function runComponentMenu(action: "copy" | "remove") {
  const target = componentMenu.value;
  closeComponentMenu();
  if (!target) return;
  if (action === "copy") emit("copy-component", target.name);
  else removeComponent(target.name);
}

onMounted(() => {
  window.addEventListener("pointerdown", closeComponentMenu);
  window.addEventListener("scroll", closeComponentMenu, true);
  window.addEventListener("blur", closeComponentMenu);
  window.addEventListener("resize", closeComponentMenu);
});
onUnmounted(() => {
  window.removeEventListener("pointerdown", closeComponentMenu);
  window.removeEventListener("scroll", closeComponentMenu, true);
  window.removeEventListener("blur", closeComponentMenu);
  window.removeEventListener("resize", closeComponentMenu);
});

/** 每个组件一条请求序号，只应用最新一次反射写入的结果。 */
const reflectSeq = new Map<string, number>();

/** 离散操作的唯一编辑键（不做撤销合并）。 */
let opSeq = 0;
function opKey(label: string): string {
  opSeq += 1;
  return `entity.${props.index}/${label}#op${opSeq}`;
}

/** 字段编辑键：同键且间隔 <700ms 的连续输入合并为一次撤销。 */
function fieldKey(path: string): string {
  return `entity.${props.index}/${path}`;
}

function shortName(name: string): string {
  return name.split("::").pop() ?? name;
}

function patch(part: Partial<EntityJson>, editKey: string) {
  emit("update", { ...props.entity, ...part }, editKey);
}

function setName(event: Event) {
  patch({ name: (event.target as HTMLInputElement).value }, fieldKey("name"));
}

const isActive = computed(() => {
  const val = props.entity.components["prism_engine::scene::Active"] ?? props.entity.components["Active"];
  return val !== false;
});

function toggleActive() {
  setComponent("prism_engine::scene::Active", !isActive.value, "Active", true);
}

const tagOptions: DropdownOption[] = [{ value: "Untagged", label: "Untagged" }];
const layerOptions: DropdownOption[] = [{ value: "Default", label: "Default" }];

const parentOptions = computed<DropdownOption[]>(() => {
  const options: DropdownOption[] = [{ value: -1, label: t("inspector.noParent") }];
  props.entities.forEach((e, i) => {
    const isSelf = i === props.index || (e.id !== undefined && e.id === props.entity.id);
    if (!isSelf) {
      options.push({
        value: e.id ?? i,
        label: `${e.name || `#${i}`} · #${i}`,
      });
    }
  });
  return options;
});

function onParentSelect(raw: number) {
  patch({ parent: Number.isFinite(raw) && raw >= 0 ? raw : null }, fieldKey("parent"));
}

function setTransform(
  key: "position" | "rotation" | "scale",
  value: JsonValue,
  path: string,
  discrete = false,
) {
  patch(
    { transform: { ...props.entity.transform, [key]: value as number[] } },
    discrete ? opKey(path) : fieldKey(path),
  );
}

// ── 欧拉角转换（顺规） ────────────────────────────────────────────────
const eulerRotation = computed(() => {
  const rot = props.entity.transform.rotation ?? [0, 0, 0, 1];
  return quaternionToEuler(rot[0] ?? 0, rot[1] ?? 0, rot[2] ?? 0, rot[3] ?? 1);
});

function setEulerRotation(val: JsonValue, _path: string, discrete = false) {
  if (!Array.isArray(val) || val.length < 3) return;
  const [pitch, yaw, roll] = val as [number, number, number];
  const q = eulerToQuaternion(pitch, yaw, roll);
  setTransform("rotation", q, "transform.rotation", discrete);
}

function componentValue(name: string): JsonValue {
  return props.entity.components[name] as JsonValue;
}

function setComponent(name: string, value: JsonValue, path: string, discrete = false) {
  const before = componentValue(name);
  const label = `components.${name}/${path}`;
  emit(
    "update",
    { ...props.entity, components: { ...props.entity.components, [name]: value } },
    discrete ? opKey(label) : fieldKey(label),
  );
  void reflectComponent(name, before, path, value, props.index);
}

async function reflectComponent(
  name: string,
  before: JsonValue,
  path: string,
  after: JsonValue,
  index: number,
) {
  const seq = (reflectSeq.get(name) ?? 0) + 1;
  reflectSeq.set(name, seq);
  try {
    const canonical = await api.setComponentField(name, before, path, after);
    if (reflectSeq.get(name) === seq) emit("reflect", name, canonical, index, after);
  } catch (e) {
    if (reflectSeq.get(name) === seq) emit("rejected", `${e}`);
  }
}

function removeComponent(name: string) {
  const components = { ...props.entity.components };
  delete components[name];
  emit("update", { ...props.entity, components }, opKey(`remove-component:${name}`));
}

function addComponent() {
  const name = addName.value;
  if (!name) return;
  const type = props.schema.types.find((t) => t.name === name);
  const value = (type?.defaultJson as JsonValue) ?? {};
  emit(
    "update",
    { ...props.entity, components: { ...props.entity.components, [name]: value } },
    opKey(`add-component:${name}`),
  );
  addName.value = "";
}

function selectAddComponent(name: string) {
  addName.value = name;
  addComponent();
  showAddPopup.value = false;
  addFilter.value = "";
}

function componentField(name: string): FieldSchema | null {
  if (!props.schema.types.some((t) => t.name === name)) return null;
  return {
    name,
    kind: "struct",
    structType: name,
    inner: null,
    arrayLen: null,
    variants: null,
    readOnly: false,
  };
}

function vecField(name: string, length: number): FieldSchema {
  return {
    name,
    kind: "array",
    structType: null,
    arrayLen: length,
    variants: null,
    readOnly: false,
    inner: {
      name: "",
      kind: "f32",
      structType: null,
      inner: null,
      arrayLen: null,
      variants: null,
      readOnly: false,
    },
  };
}

const componentNames = computed(() => Object.keys(props.entity.components));

function matchedFields(name: string): string[] {
  const query = filter.value.trim().toLowerCase();
  if (!query) return [];
  const type = props.schema.types.find((t) => t.name === name);
  return (type?.fields ?? [])
    .map((field) => field.name)
    .filter((field) => field.toLowerCase().includes(query));
}

function matchesFilter(name: string): boolean {
  const query = filter.value.trim().toLowerCase();
  if (!query) return true;
  if (name.toLowerCase().includes(query) || shortName(name).toLowerCase().includes(query)) {
    return true;
  }
  if (matchedFields(name).length) return true;
  const value = JSON.stringify(componentValue(name) ?? "").toLowerCase();
  return value.includes(query);
}

const visibleComponents = computed(() => componentNames.value.filter(matchesFilter));

const transformMatches = computed(() => {
  const query = filter.value.trim().toLowerCase();
  if (!query) return true;
  return JSON.stringify(props.entity.transform).toLowerCase().includes(query)
    || "transform position rotation scale".includes(query);
});

const availableComponents = computed(() =>
  props.schema.types
    .filter((t) => t.isComponent && !(t.name in props.entity.components))
    .map((t) => t.name),
);

const filteredAvailableComponents = computed(() => {
  const query = addFilter.value.trim().toLowerCase();
  if (!query) return availableComponents.value;
  return availableComponents.value.filter((n) =>
    n.toLowerCase().includes(query) || shortName(n).toLowerCase().includes(query),
  );
});
</script>

<template>
  <!-- GameObject Header -->
  <div class="gameobject-header">
    <div class="gameobject-row-top">
      <input
        type="checkbox"
        class="entity-active-check"
        :checked="isActive"
        :title="$t('inspector.activeTooltip')"
        @change="toggleActive"
      />
      <input
        type="text"
        class="entity-name-input"
        :value="entity.name ?? ''"
        :placeholder="$t('inspector.namePlaceholder')"
        @input="setName"
      />
      <label class="static-label" :title="$t('inspector.staticTooltip')">
        <input type="checkbox" disabled />
        <span>{{ $t("inspector.static") }}</span>
      </label>
    </div>
    <div class="gameobject-row-bottom">
      <div class="tag-layer-cell">
        <span class="sub-label">{{ $t("inspector.tag") }}</span>
        <EditorDropdown model-value="Untagged" :options="tagOptions" disabled block />
      </div>
      <div class="tag-layer-cell">
        <span class="sub-label">{{ $t("inspector.layer") }}</span>
        <EditorDropdown model-value="Default" :options="layerOptions" disabled block />
      </div>
    </div>
  </div>

  <!-- 检索过滤栏 -->
  <div style="padding: 4px 8px; background: var(--chrome); border-bottom: 1px solid var(--border)">
    <div class="scalar" style="height: 20px">
      <input
        v-model="filter"
        type="text"
        :placeholder="$t('inspector.searchPlaceholder')"
        style="height: 19px"
      />
      <button v-if="filter" class="ghost icon" :title="$t('common.clear')" @click="filter = ''">
        <X :size="12" />
      </button>
    </div>
  </div>

  <!-- 父实体选择 -->
  <div class="card" style="margin-bottom: 2px">
    <div class="card-body" style="padding: 3px 8px">
      <div class="row">
        <div class="label" :title="$t('inspector.parentTooltip')">{{ $t("inspector.parent") }}</div>
        <div>
          <EditorDropdown
            :model-value="entity.parent ?? -1"
            :options="parentOptions"
            block
            @update:model-value="(val) => onParentSelect(Number(val))"
          />
        </div>
      </div>
    </div>
  </div>

  <!-- 预制体实例指示卡片 -->
  <div v-if="prefabLink" class="card prefab-link">
    <header>
      <span class="title">{{ $t("inspector.prefabInstance") }}</span>
      <span class="hint" :title="prefabLink">{{ prefabLink.split('/').pop() }}</span>
      <span class="spacer" />
      <button class="ghost icon" :title="$t('inspector.revertPrefabTooltip')" @click="emit('revert-prefab', index)">
        {{ $t("inspector.revertPrefab") }}
      </button>
      <button
        class="ghost icon"
        :title="$t('inspector.applyPrefabTooltip')"
        @click="emit('apply-prefab', index)"
      >
        {{ $t("inspector.applyPrefab") }}
      </button>
      <button class="ghost icon" :title="$t('inspector.unlinkPrefabTooltip')" @click="emit('unlink-prefab', index)">
        {{ $t("inspector.unlinkPrefab") }}
      </button>
    </header>
  </div>

  <!-- 剪贴板条目 -->
  <div v-if="clipboard" class="card clipboard">
    <div class="card-body" style="display: flex; align-items: center; gap: 6px; padding: 4px 8px">
      <span class="hint">{{ $t("inspector.clipboard") }}: {{ shortName(clipboard.component) }}</span>
      <span class="spacer" />
      <button class="primary" title="Ctrl+V" @click="emit('paste-component')">
        {{ $t("inspector.pasteToSelection", { count: selectionCount }) }}
      </button>
      <button class="ghost icon" @click="emit('clear-clipboard')"><X :size="12" /></button>
    </div>
  </div>

  <!-- Transform 变换组件 -->
  <div v-if="transformMatches" class="card">
    <header @click="toggleCard('Transform')">
      <span class="foldout-icon">
        <ChevronDown v-if="!collapsedCards.has('Transform')" :size="12" />
        <ChevronRight v-else :size="12" />
      </span>
      <span class="title">Transform</span>
      <span class="spacer" />
      <button
        class="ghost icon"
        style="width: auto; padding: 0 4px; font-size: 10px; height: 18px"
        :title="$t('inspector.toggleRotationMode')"
        @click.stop="showEuler = !showEuler"
      >
        {{ showEuler ? "Euler°" : "Quat" }}
      </button>
    </header>
    <div v-if="!collapsedCards.has('Transform')" class="card-body">
      <FieldEditor
        label="Position"
        path="transform.position"
        :value="entity.transform.position"
        :field="vecField('position', 3)"
        :schema="schema"
        @update="(v, p, d) => setTransform('position', v, p, d)"
      />
      <FieldEditor
        v-if="showEuler"
        label="Rotation"
        path="transform.rotation"
        :value="eulerRotation"
        :field="vecField('rotation', 3)"
        :schema="schema"
        @update="(v, p, d) => setEulerRotation(v, p, d)"
      />
      <FieldEditor
        v-else
        label="Rotation (Q)"
        path="transform.rotation"
        :value="entity.transform.rotation"
        :field="vecField('rotation', 4)"
        :schema="schema"
        @update="(v, p, d) => setTransform('rotation', v, p, d)"
      />
      <FieldEditor
        label="Scale"
        path="transform.scale"
        :value="entity.transform.scale"
        :field="vecField('scale', 3)"
        :schema="schema"
        @update="(v, p, d) => setTransform('scale', v, p, d)"
      />
    </div>
  </div>

  <!-- 挂载的各个组件卡片 -->
  <div
    v-for="name in visibleComponents"
    :key="name"
    class="card"
    :class="{ highlight: highlightComponent === name }"
  >
    <header
      @click="toggleCard(name)"
      @contextmenu.prevent.stop="openComponentMenu(name, $event)"
    >
      <span class="foldout-icon">
        <ChevronDown v-if="!collapsedCards.has(name)" :size="12" />
        <ChevronRight v-else :size="12" />
      </span>
      <span class="title" :title="name">{{ shortName(name) }}</span>
      <span class="spacer" />
      <button
        class="menu-icon-btn"
        :title="$t('inspector.componentMenu')"
        @click.stop="openComponentMenu(name, $event)"
      >
        <MoreVertical :size="13" />
      </button>
    </header>
    <div v-if="!collapsedCards.has(name)" class="card-body">
      <div v-if="filter && matchedFields(name).length" class="hint" style="margin-bottom: 4px">
        {{ $t("inspector.matchedFields") }}: {{ matchedFields(name).join(", ") }}
      </div>
      <div
        v-if="highlightComponent === name && highlightField"
        class="hint"
        style="margin-bottom: 4px"
      >
        {{ $t("inspector.highlightField") }}: {{ highlightField }}
      </div>
      <FieldEditor
        path=""
        :value="componentValue(name)"
        :field="componentField(name)"
        :schema="schema"
        @update="(v, p, d) => setComponent(name, v, p, d)"
      />
    </div>
  </div>

  <div v-if="filter && !visibleComponents.length" class="hint" style="padding: 12px; text-align: center">
    {{ $t("inspector.noMatchingComponents", { query: filter }) }}
  </div>

  <!--居中 Add Component 按钮 -->
  <div class="add-component-container">
    <button class="add-component-button" @click="showAddPopup = !showAddPopup">
      {{ $t("inspector.addComponent") }}
    </button>
    <div v-if="showAddPopup" class="add-component-popup" @pointerdown.stop>
      <div class="add-component-filter">
        <input
          v-model="addFilter"
          type="text"
          :placeholder="$t('inspector.searchComponents')"
          autofocus
        />
      </div>
      <div class="add-component-list">
        <div
          v-for="n in filteredAvailableComponents"
          :key="n"
          class="add-component-item"
          @click="selectAddComponent(n)"
        >
          <span>{{ shortName(n) }}</span>
          <span style="font-size: 10px; color: var(--muted); margin-left: auto">{{ n }}</span>
        </div>
        <div v-if="!filteredAvailableComponents.length" class="hint" style="padding: 8px; text-align: center">
          {{ $t("common.none") }}
        </div>
      </div>
    </div>
  </div>

  <Teleport to="body">
    <div
      v-if="componentMenu"
      class="component-context-menu"
      :style="{ left: `${componentMenu.x}px`, top: `${componentMenu.y}px` }"
      @pointerdown.stop
      @click.stop
      @contextmenu.prevent
    >
      <button @click="runComponentMenu('copy')">
        <span>{{ $t("inspector.copyComponent") }}</span>
      </button>
      <button class="danger" @click="runComponentMenu('remove')">
        <span>{{ $t("inspector.removeComponent") }}</span>
      </button>
    </div>
  </Teleport>
</template>
