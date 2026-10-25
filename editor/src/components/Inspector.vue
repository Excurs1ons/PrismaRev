<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { X } from "lucide-vue-next";
import { clampMenuPosition } from "../contextMenu";
import FieldEditor from "./FieldEditor.vue";
import * as api from "../api";
import type { ComponentClipboard, EntityJson, FieldSchema, JsonValue, Schema } from "../types";

/**
 * 单实体检查器：name / parent / transform + 任意组件（schema 驱动）。
 *
 * - 组件字段的改动**先本地提交**（拖动/输入要即时），再交给 Rust 侧
 *   `set_component_field` 走反射写入：只读被拒绝、类型经反射校验，
 *   返回规范化后的组件 JSON 回填（`reflect` 事件）。
 * - 顶部检索框按「组件名 / 字段名 / 值」过滤组件卡片。
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
  (e: "reflect", component: string, canonical: JsonValue): void;
  (e: "rejected", message: string): void;
}>();

const addName = ref("");
const filter = ref("");
const componentMenu = ref<{ name: string; x: number; y: number } | null>(null);

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

function onParentChange(event: Event) {
  const raw = Number((event.target as HTMLSelectElement).value);
  patch({ parent: Number.isFinite(raw) && raw >= 0 ? raw : null }, fieldKey("parent"));
}

function setTransform(key: "position" | "rotation" | "scale", value: JsonValue, path: string) {
  patch({ transform: { ...props.entity.transform, [key]: value as number[] } }, fieldKey(path));
}

function componentValue(name: string): JsonValue {
  return props.entity.components[name] as JsonValue;
}

function setComponent(name: string, value: JsonValue, path: string) {
  const before = componentValue(name);
  emit(
    "update",
    { ...props.entity, components: { ...props.entity.components, [name]: value } },
    fieldKey(`components.${name}/${path}`),
  );
  void reflectComponent(name, before, path, value);
}

/** 反射写入：成功后回填规范化值；失败则报告（通常是只读或类型不合法）。 */
async function reflectComponent(name: string, before: JsonValue, path: string, after: JsonValue) {
  const seq = (reflectSeq.get(name) ?? 0) + 1;
  reflectSeq.set(name, seq);
  try {
    const canonical = await api.setComponentField(name, before, path, after);
    if (reflectSeq.get(name) === seq) emit("reflect", name, canonical);
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

/** 组件 JSON 值本身不是结构体时（`Name` 裸字符串等），仅作类型提示。 */
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

/** 合成一个定长 `f32[n]` 字段，用于 transform 三个向量。 */
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

/** 检索命中：组件名 / 字段名 / 值任一包含关键词。 */
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
</script>

<template>
  <div class="card">
    <header>
      <span class="title">实体 #{{ index }}</span>
      <span v-if="selectionCount > 1" class="hint">（已选 {{ selectionCount }}）</span>
      <span class="spacer" />
    </header>
    <div class="card-body">
      <div class="row">
        <div class="label">检索</div>
        <div class="scalar">
          <input v-model="filter" type="text" placeholder="组件名 / 字段名 / 值…" />
          <button v-if="filter" class="ghost icon" title="清除" @click="filter = ''">
            <X :size="13" />
          </button>
        </div>
      </div>
      <div class="row">
        <div class="label">name</div>
        <div>
          <input type="text" :value="entity.name ?? ''" @input="setName" />
        </div>
      </div>
      <div class="row">
        <div class="label">parent</div>
        <div>
          <select :value="entity.parent ?? -1" @change="onParentChange">
            <option value="-1">（无父）</option>
            <option v-for="(e, i) in entities" :key="i" :value="i" :disabled="i === index">
              {{ e.name || `#${i}` }} · #{{ i }}
            </option>
          </select>
        </div>
      </div>
    </div>
  </div>

  <div v-if="prefabLink" class="card prefab-link">
    <header>
      <span class="title">预制体实例</span>
      <span class="hint">{{ prefabLink }}</span>
      <span class="spacer" />
      <button class="ghost icon" title="用预制体的当前内容覆盖这个实例" @click="emit('revert-prefab', index)">
        还原
      </button>
      <button
        class="ghost icon"
        title="把这个实例（含子级）的内容写回预制体文件"
        @click="emit('apply-prefab', index)"
      >
        应用到预制体
      </button>
      <button class="ghost icon" title="解除链接（只去掉记录）" @click="emit('unlink-prefab', index)">
        解除
      </button>
    </header>
  </div>

  <div v-if="clipboard" class="card clipboard">
    <div class="card-body">
      <span class="hint">剪贴板：{{ shortName(clipboard.component) }}</span>
      <span class="spacer" />
      <button class="primary" title="Ctrl+V" @click="emit('paste-component')">
        粘贴到选中（{{ selectionCount }}）
      </button>
      <button class="ghost icon" @click="emit('clear-clipboard')"><X :size="13" /></button>
    </div>
  </div>

  <div v-if="transformMatches" class="card">
    <header>
      <span class="title">transform</span>
      <span class="hint">局部 · T×R×S</span>
      <span class="spacer" />
    </header>
    <div class="card-body">
      <FieldEditor
        label="position"
        path="transform.position"
        :value="entity.transform.position"
        :field="vecField('position', 3)"
        :schema="schema"
        @update="(v, p) => setTransform('position', v, p)"
      />
      <FieldEditor
        label="rotation"
        path="transform.rotation"
        :value="entity.transform.rotation"
        :field="vecField('rotation', 4)"
        :schema="schema"
        @update="(v, p) => setTransform('rotation', v, p)"
      />
      <FieldEditor
        label="scale"
        path="transform.scale"
        :value="entity.transform.scale"
        :field="vecField('scale', 3)"
        :schema="schema"
        @update="(v, p) => setTransform('scale', v, p)"
      />
    </div>
  </div>

  <div
    v-for="name in visibleComponents"
    :key="name"
    class="card"
    :class="{ highlight: highlightComponent === name }"
  >
    <header @contextmenu.prevent.stop="openComponentMenu(name, $event)">
      <span class="title">{{ shortName(name) }}</span>
      <span class="hint">{{ name }}</span>
      <span class="spacer" />
    </header>
    <div class="card-body">
      <div v-if="filter && matchedFields(name).length" class="hint" style="margin-bottom: 4px">
        匹配字段：{{ matchedFields(name).join("、") }}
      </div>
      <div
        v-if="highlightComponent === name && highlightField"
        class="hint"
        style="margin-bottom: 4px"
      >
        定位字段：{{ highlightField }}
      </div>
      <FieldEditor
        path=""
        :value="componentValue(name)"
        :field="componentField(name)"
        :schema="schema"
        @update="(v, p) => setComponent(name, v, p)"
      />
    </div>
  </div>

  <div v-if="filter && !visibleComponents.length" class="hint" style="padding: 8px">
    没有匹配「{{ filter }}」的组件
  </div>

  <div class="card">
    <header><span class="title">添加组件</span></header>
    <div class="card-body">
      <div style="display: flex; gap: 8px">
        <select v-model="addName">
          <option value="">选择类型…</option>
          <option v-for="n in availableComponents" :key="n" :value="n">{{ shortName(n) }}</option>
        </select>
        <button class="primary" :disabled="!addName" @click="addComponent">添加</button>
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
        <span>复制组件</span>
      </button>
      <button class="danger" @click="runComponentMenu('remove')">
        <span>移除组件</span>
      </button>
    </div>
  </Teleport>
</template>
