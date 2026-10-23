<script setup lang="ts">
import { computed, ref } from "vue";
import FieldEditor from "./FieldEditor.vue";
import * as api from "../api";
import type { EntityJson, FieldSchema, JsonValue, Schema } from "../types";

/**
 * 单实体检查器：name / parent / transform + 任意组件（schema 驱动）。
 *
 * 组件字段的改动**先本地提交**（拖动/输入要即时），再交给 Rust 侧
 * `set_component_field` 走反射写入：只读字段会被拒绝、值经反射类型校验，
 * 返回规范化后的组件 JSON 回填（`reflect` 事件）。
 */
const props = defineProps<{
  entity: EntityJson;
  index: number;
  entities: EntityJson[];
  schema: Schema;
}>();

const emit = defineEmits<{
  (e: "update", entity: EntityJson, editKey: string): void;
  (e: "remove", index: number): void;
  (e: "duplicate", index: number): void;
  (e: "reflect", component: string, canonical: JsonValue): void;
  (e: "rejected", message: string): void;
}>();

const addName = ref("");

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

function setTransform(key: "translation" | "rotation" | "scale", value: JsonValue, path: string) {
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
async function reflectComponent(
  name: string,
  before: JsonValue,
  path: string,
  after: JsonValue,
) {
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
  return { name, kind: "struct", structType: name, inner: null, arrayLen: null, readOnly: false };
}

/** 合成一个定长 `f32[n]` 字段，用于 transform 三个向量。 */
function vecField(name: string, length: number): FieldSchema {
  return {
    name,
    kind: "array",
    structType: null,
    arrayLen: length,
    readOnly: false,
    inner: { name: "", kind: "f32", structType: null, inner: null, arrayLen: null, readOnly: false },
  };
}

const componentNames = computed(() => Object.keys(props.entity.components));

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
      <span class="spacer" />
      <button class="ghost icon" title="复制实体 (Ctrl+D)" @click="emit('duplicate', index)">
        复制
      </button>
      <button
        class="ghost icon"
        :disabled="entities.length <= 1"
        title="删除实体（至少保留一个，Delete）"
        @click="emit('remove', index)"
      >
        删除
      </button>
    </header>
    <div class="card-body">
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
    <div class="card-body" style="border-top: 1px solid var(--border)">
      <div class="hint" style="margin-bottom: 4px">transform（局部 · T×R×S）</div>
      <FieldEditor
        label="translation"
        path="transform.translation"
        :value="entity.transform.translation"
        :field="vecField('translation', 3)"
        :schema="schema"
        @update="(v, p) => setTransform('translation', v, p)"
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

  <div v-for="name in componentNames" :key="name" class="card">
    <header>
      <span class="title">{{ shortName(name) }}</span>
      <span class="hint">{{ name }}</span>
      <span class="spacer" />
      <button class="ghost icon" title="移除组件" @click="removeComponent(name)">移除</button>
    </header>
    <div class="card-body">
      <FieldEditor
        path=""
        :value="componentValue(name)"
        :field="componentField(name)"
        :schema="schema"
        @update="(v, p) => setComponent(name, v, p)"
      />
    </div>
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
</template>
